//! Values crossing the connector top's parent boundary. Durable records are
//! whole rows: the parent saves them in the decision and only then sends
//! `Committed` for a new outbox entry (domain/connectors.md, 4.2).
use alloc::boxed::Box;
use skein_lib::Token;
use temper_engine_domain_forge_change as change;
use temper_engine_domain_forge_client as client;
use temper_engine_domain_forge_issues as issues;

/// A resource of one configured forge repository (domain/forge.md, section 2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Name {
    pub forge: u16,
    pub repository: u32,
    pub what: What,
}
/// The kind and identity within a repository.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum What {
    /// The repository itself.
    Repository,
    /// A branch's path segments.
    Branch(Box<[Box<[u8]>]>),
    /// A numbered pull request.
    Pull(u64),
    /// A numbered issue.
    Issue(u64),
}
/// What the project may do on an adopted repository.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Temper owns the repository.
    Owned,
    /// Temper may contribute and land in it.
    Adopted,
    /// Temper contributes through a fork.
    Fork,
    /// Temper reads but never writes.
    Context,
}
/// Capabilities observed when the repository was adopted, narrowed on refusal.
#[expect(clippy::struct_excessive_bools, reason = "the forge has independent effect kinds")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Kinds {
    pub read: bool,
    pub push: bool,
    pub open: bool,
    pub land: bool,
    pub review: bool,
    pub status: bool,
    pub comment: bool,
    pub issue: bool,
    pub branch: bool,
}
/// One adopted repository, with provider name and project ownership.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Repository {
    pub project: u32,
    /// This project's selected repository for goal issue projections and chats' saved work.
    pub home: bool,
    pub provider: client::api::Repository,
    pub host: Box<[u8]>,
    pub owner: Box<[u8]>,
    pub name: Box<[u8]>,
    pub prefix: Box<[u8]>,
    pub role: Role,
    /// Whether this repository runs CI; absence of a status never selects this policy.
    pub ci: bool,
    /// Blocking project check gates used when CI is absent.
    pub checks: Box<[u32]>,
    pub kinds: Kinds,
    pub protection: Protection,
    pub settings: client::api::Settings,
}
/// Protection as observed by this writer at adoption.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Protection {
    /// The writer could not read protection.
    Unknown,
    /// No applicable rule exists.
    Absent,
    /// An applicable rule was read.
    Rule(client::api::Protection),
}
/// The caller's request to adopt an existing repository.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Adoption {
    pub project: u32,
    /// Select this repository as the project's home if it has none yet.
    pub home: bool,
    pub provider: client::api::Repository,
    pub host: Box<[u8]>,
    pub owner: Box<[u8]>,
    pub name: Box<[u8]>,
    pub prefix: Box<[u8]>,
    pub role: Role,
    pub landing: Box<[u8]>,
    pub ci: bool,
    pub checks: Box<[u32]>,
}
/// The admitted repository and its observed collaborators.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Adopted {
    pub repository: Repository,
    pub collaborators: Box<[client::api::Collaborator]>,
}
/// Exclusive write hold and its current writer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hold {
    pub name: Name,
    pub task: u64,
    pub writer: Option<Writer>,
}
/// Durable progress while a closing task releases its connector resources.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReleaseRow {
    pub task: u64,
    pub root: u64,
    pub ending: ReleaseEnding,
    pub close_pull: Option<u64>,
    pub pending: Option<(u64, Option<Name>)>,
    pub failed: bool,
}
/// Terminal task class that determines forge cleanup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReleaseEnding {
    /// The task finished successfully.
    Done,
    /// The task failed and its branch remains until the root closes.
    Failed,
    /// The task was cancelled, so its pull closes before its branch is deleted.
    Cancelled,
}
/// A run or connector effect occupying one writer slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Writer {
    /// A worker attempt, fenced by the root's activation.
    Run { task: u64, attempt: u64 },
    /// An outbox entry making a change to the resource.
    Entry(u64),
}
/// A subject published by the forge (domain/forge.md, section 7).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Topic {
    /// A branch's tip moved.
    Landings { repository: client::api::Repository, branch: Box<[u8]> },
    /// CI changed on one commit.
    Ci { repository: client::api::Repository, head: client::api::Commit },
    /// A pull's state changed.
    Pull { repository: client::api::Repository, number: u64 },
    /// A participant wrote on an object.
    Participation { repository: client::api::Repository, number: u64 },
}
/// A subscriber's classification of observed news.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// Start or resume this task when its inbox permits it.
    Wakes,
    /// Retain for the next run.
    Kept,
    /// The task has no need to hear this event.
    Dropped,
}
/// News delivered through a topic to task inboxes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum News {
    /// A branch moved; overlap is decided from bounded file facts.
    Landing { before: client::api::Commit, after: client::api::Commit, files: Option<Box<[Box<[u8]>]>> },
    /// The combined CI state changed at its head.
    Ci { head: client::api::Commit, status: client::api::Ci },
    /// A pull or participating object's state changed.
    Changed { number: u64 },
}
/// A live subscription and its known paths for landing overlap.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Subscriber {
    pub task: u64,
    /// Root-issued task subscription, retained across restarts for news delivery.
    pub number: u64,
    pub topic: Topic,
    pub own_change: Option<u64>,
    pub paths: Box<[Box<[u8]>]>,
}
/// Last observed branch tip, used to publish each landing once.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BranchHead {
    pub name: Name,
    pub commit: client::api::Commit,
}
/// Last published pull state and CI verdict.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PullState {
    pub name: Name,
    pub head: client::api::Commit,
    pub ci: client::api::Ci,
    pub state: client::api::State,
}
/// Last CI verdict published to subscribers of one commit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CiState {
    pub repository: client::api::Repository,
    pub head: client::api::Commit,
    pub status: client::api::Ci,
}
/// A goal's issue projection, including an in-flight write and created issue.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IssueRow {
    pub goal: u64,
    pub repository: client::api::Repository,
    pub state: issues::Projected,
    pub number: Option<u64>,
    pub pending: Option<u64>,
    pub before: Option<issues::Projected>,
    pub failed: bool,
    /// Latest root-authorized goal snapshot, including an ending while a write is pending.
    pub desired: Option<issues::GoalView>,
}
/// A change task's forge parameters and top-owned procedure state.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChangeRow {
    pub task: u64,
    pub repository: client::api::Repository,
    pub branch: Box<[u8]>,
    pub base: Box<[u8]>,
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
    pub priority: i32,
    pub change: change::Change,
    pub pull: Option<u64>,
    pub pending: Option<u64>,
    pub effect: change::EffectResult,
    /// The one child requested by this procedure, and its latest result.
    pub delegate: Option<(u64, change::Delegate)>,
    pub delegate_status: change::Status,
    /// Bounded current verdict per gate, retained through clean updates.
    pub verdicts: Box<[change::GateReport]>,
    /// Bounded explanations of gate verdicts at their named heads.
    pub gate_remarks: Box<[GateRemark]>,
    pub drift: Option<change::Hold>,
    /// A deployment-owned repair of a landing branch, exempt from that branch's broken CI.
    pub base_repair: bool,
    /// The base repair started while this ordinary change owned the queue.
    pub queue_repair: Option<u64>,
    /// Repairs attempted for this landing queue while this change owned it.
    pub queue_repairs: u32,
}
/// One gate's explanation, retained with its verdict for a repair brief.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GateRemark {
    pub number: u64,
    pub head: client::api::Commit,
    pub words: Box<[u8]>,
}
/// Coherent provider facts used by the root to check a change effect before
/// releasing its durable outbox entry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChangeEvidence {
    /// State to restore if the root does not authorize this effect.
    pub prior: change::Change,
    pub head: Option<client::api::Commit>,
    pub base_tip: client::api::Commit,
    pub contains_base: change::Status,
    pub ci: change::Status,
    pub gates: Box<[change::GateReport]>,
    pub reviews: Box<[client::api::Review]>,
    pub reviews_complete: bool,
}
/// The connector's durable records, wrapped by the root store.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Stored {
    /// One adopted repository.
    Repository(Repository),
    /// One held resource.
    Hold(Hold),
    /// All live resources named by one task.
    Names {
        task: u64,
        resources: Box<[Name]>,
    },
    /// One topic subscription.
    Subscription(Subscriber),
    /// One branch's last observed tip.
    BranchHead(BranchHead),
    /// One pull's last published state.
    PullState(PullState),
    Ci(CiState),
    /// A merge made by this deployment, for own-change news classification.
    Landed {
        commit: client::api::Commit,
        task: u64,
    },
    /// An unsettled outbox entry.
    Entry(client::Entry),
    /// A client working-set row.
    Client(client::Stored),
    /// A change procedure's committed state.
    Change(ChangeRow),
    /// A goal issue's committed projection.
    Issue(IssueRow),
    Release(ReleaseRow),
}
/// Stable store key of a connector record.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    Repository(client::api::Repository),
    Hold(Name),
    Names(u64),
    Subscription { task: u64, topic: Topic },
    BranchHead(Name),
    PullState(Name),
    Ci { repository: client::api::Repository, head: client::api::Commit },
    Landed(client::api::Commit),
    Entry(u64),
    Client(client::Key),
    Change(u64),
    Issue(u64),
    Release(u64),
}
/// A parent or child input to the connector top.
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Read current permissions, settings, protection and collaborators.
    Adopt { reply_to: Token, adoption: Adoption },
    /// Withdraw a just-adopted repository when its parent cannot commit the role seed.
    ForgetAdoption { repository: client::api::Repository, restore: Option<Repository> },
    /// Replace the resources this live task names.
    Names { task: u64, resources: Box<[Name]> },
    /// Remove all resources and subscriptions of an ended task.
    Unnamed { task: u64 },
    /// Take a hold or hand one down from its current holder.
    Hold { task: u64, resource: Name, from: Option<u64> },
    /// Claim writer slots for one task's attempt.
    Claim { task: u64, attempt: u64, writes: Box<[Name]>, holders: Box<[u64]> },
    /// Release slots and confirm any pushed heads.
    Answered { task: u64, attempt: u64, pushed: Box<[(Name, client::api::Commit)]> },
    /// Read a lost writer's resource afresh before making its slot available.
    Lost { task: u64, attempt: u64 },
    /// Enqueue a checked effect in the same decision as its cause.
    Enqueue { entry: client::Entry },
    /// Project a goal after the root checked its issue authority.
    Project { entry: u64, repository: client::api::Repository, view: issues::GoalView },
    /// Continue the latest authorized snapshot after an outbox outcome or interval.
    ProjectDesired { entry: u64, goal: u64 },
    /// Register one activated change task with its held branch.
    Change { row: ChangeRow },
    /// Commit the deployment repair identity beside the task creation decision.
    QueueRepairStarted { owner: u64, repair: u64 },
    /// Remember a committed procedure child and its role in this change.
    Delegated { task: u64, child: u64, kind: change::Delegate },
    /// A proposed child was refused by tasks before creation; keep a releasable state.
    DelegateRefused { task: u64, child: u64 },
    /// Resolve the named child's terminal result before stepping again.
    DelegateResult { task: u64, child: u64, status: change::Status, words: Box<[u8]> },
    /// Gather fresh forge facts and step one registered change.
    StepChange {
        task: u64,
        entry: u64,
        heard: change::Heard,
        gates: Box<[change::GateReport]>,
        queue_repair_active: bool,
    },
    /// A person released a drift-held change after inspecting the new head.
    ReleaseChange { task: u64 },
    /// Finish a closing task's prior effects, then release its holds.
    Release { task: u64, root: u64, ending: ReleaseEnding, entry: u64 },
    /// Continue a release after one of its outbox effects settles.
    ContinueRelease { task: u64, entry: u64 },
    /// Lift a held projection after a person resolves its permanent failure.
    ReleaseProjection { goal: u64 },
    /// The root has durably committed this entry.
    Committed { entry: u64 },
    /// Withdraw an entry whose write has not gone out.
    Withdraw { entry: u64 },
    /// Root authority vetoed a new, unsent change effect in this decision.
    VetoChange { task: u64, entry: u64, prior: change::Change },
    /// Subscribe a live task to a topic.
    Subscribe { subscription: Subscriber },
    /// Unsubscribe a task from a topic.
    Unsubscribe { task: u64, topic: Topic },
    /// One provider hint passed from the protocol.
    Hint { hint: client::api::Hint },
    /// A client event returned after an earlier connector request.
    Client(client::Event),
    /// Restore one durable connector record.
    Restore { record: Stored },
    /// Finish restoration and reconcile loaded entries.
    Restored { clock: client::RecoveryClock },
}
/// The connector's output to the root, including child API calls.
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Terminal result of a repository adoption, for the root's person route.
    Adopted { reply_to: Token, result: Result<Adopted, client::api::Error> },
    /// Save one record atomically with the current decision.
    Save { record: Stored },
    /// Erase one record atomically with the current decision.
    Erase { key: Key },
    /// A resource is held or a writer slot is taken.
    Taken { task: u64, resource: Name, by: u64 },
    /// A request exceeds the connector's configured bounds.
    Refused { task: u64 },
    /// An entry settled or remains uncertain; root routes the task's news.
    Outcome { entry: u64, task: u64, outcome: client::Outcome },
    /// A prior effect or release write settled; advance the closing task.
    ContinueRelease { task: u64 },
    /// All connector resources of a closing task are released.
    Released { task: u64 },
    /// A cleanup effect failed and the closing task needs a person's decision.
    ReleaseFailed { task: u64 },
    /// A projection has to wait until its interval or prior effect settles.
    ProjectAfter { goal: u64, when: Option<skein_lib::Wall> },
    /// A projection could not continue under current facts or limits.
    ProjectionFailed { goal: u64 },
    /// A change policy step and its checked effect entry, when any.
    ChangeDecision { task: u64, decision: change::Decision, entry: Option<u64>, evidence: Option<ChangeEvidence> },
    /// Task-specific news with a connector classification.
    News { task: u64, subscription: u64, topic: Topic, news: News, class: Class },
    /// Drift holds this task until a person releases it.
    Drift { task: u64, resource: Name },
    /// A bounded provider call, released after preceding progress commits.
    Call { call: Token, repository: client::api::Repository, op: client::api::Op },
    /// One bounded fresh connector read for a root worker tool.
    Read { owner: Token, result: Result<client::api::Answer, client::api::Error> },
}
