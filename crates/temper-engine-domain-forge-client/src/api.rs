//! Forge API values passed between this client and its protocol adapter
//! (domain/forge.md, sections 5, 6 and 20.1).
//!
//! These types describe bounded requests and answers; they retain no state.
//! They never name tasks or authorization policy. The adapter maps a `Read`
//! or `Write` to provider calls and returns one typed terminal. A job-log
//! read names one run, job, attempt, and head, and returns at most its byte cap.
use alloc::boxed::Box;
use skein_lib::{Duration, Time};

/// A forge commit identifier in the adapter’s fixed-width form.
pub type Commit = [u8; 32];
/// A repository named within one configured forge.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Repository {
    pub forge: u16,
    pub repository: u32,
}
/// A provider hint that may advance a live-resource read.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Hint {
    pub repository: Repository,
    pub change: Change,
    pub key: Option<Box<[u8]>>,
}
/// The resource named by a provider hint.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Change {
    /// A hint about an item.
    Item(u64),
    /// A hint about a branch.
    Branch(Box<[u8]>),
    /// An authenticated push webhook. The hint advances a fresh branch read;
    /// its actor is used only if that read still observes this head.
    BranchMoved { branch: Box<[u8]>, head: Commit, actor: u64 },
    /// A hint about a commit.
    Commit(Commit),
}
/// A typed call the protocol adapter makes to the forge.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Op {
    /// A read operation.
    Read(Read),
    /// A write operation.
    Write(Write),
}
/// A typed forge read, bounded by the client’s limits.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Read {
    /// Inclusive forge time, least recently updated first, pages from 1.
    Items {
        /// Inclusive at the provider's time resolution. Saved positions come
        /// from observed forge timestamps; arbitrary caller times may overlap
        /// the preceding coarse quantum. Page validation conservatively
        /// tolerates that overlap rather than claiming adapter normalization.
        since: Time,
        page: u32,
        kind: Option<Kind>,
    },
    /// Read item.
    Item { number: u64, after: u64 },
    /// Read pull.
    Pull { number: u64 },
    /// Newest matching pull, open or closed.
    PullFor { head: Box<[u8]>, base: Box<[u8]> },
    /// Read reviews.
    Reviews { number: u64, page: u32 },
    /// Read statuses.
    Statuses { commit: Commit, page: u32 },
    /// Read remarks.
    Remarks { number: u64, review: u64, page: u32 },
    /// Read branch.
    Branch { branch: Box<[u8]> },
    /// Names of the repository's branches, bounded by the adapter's answer limit.
    Branches,
    /// Read pull files.
    PullFiles { number: u64, head: Commit, page: u32 },
    /// Whole bounded comparison; `TooLarge` means unknown overlap.
    Compare { before: Commit, after: Commit },
    /// Provider descriptions and links, with a failing job's attempt.
    Checks { commit: Commit },
    /// Bounded plaintext log for one failed job attempt at its head.
    Job { attempt: JobAttempt, max_bytes: u32 },
    /// One file at an immutable head, cut to the requested byte budget.
    File { head: Commit, path: Box<[u8]>, max_bytes: u32 },
    /// Requires admin, including an absent rule.
    Protection { branch: Box<[u8]> },
    /// Read settings.
    Settings,
    /// Read collaborators.
    Collaborators { page: u32 },
    /// Read permission.
    Permission { user: u64 },
}
/// A typed forge write handed down from a committed outbox entry.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Write {
    /// Write create issue.
    CreateIssue { key: Box<[u8]>, title: Box<[u8]>, body: Box<[u8]> },
    /// Write open pull.
    OpenPull { title: Box<[u8]>, body: Box<[u8]>, head: Box<[u8]>, base: Box<[u8]> },
    /// Write post.
    Post { number: u64, key: Box<[u8]>, body: Box<[u8]> },
    /// Write review.
    Review { number: u64, key: Box<[u8]>, verdict: Verdict, body: Box<[u8]> },
    /// Write edit.
    Edit { number: u64, title: Option<Box<[u8]>>, body: Option<Box<[u8]>> },
    /// Write set reviewers.
    SetReviewers { number: u64, reviewers: Box<[u64]> },
    /// Write close.
    Close { number: u64 },
    /// Write reopen.
    Reopen { number: u64 },
    /// Checked at exact head. The outbox effect's intended base is a
    /// client-side precondition, checked by reading the pull afresh.
    Merge { number: u64, head: Commit },
    /// Forge-side merge update, with two parents and fresh CI.
    Update { number: u64 },
    /// Write status.
    Status { commit: Commit, context: Box<[u8]>, check: Check },
    /// Write create branch.
    CreateBranch { branch: Box<[u8]>, commit: Commit },
    /// Write delete branch.
    DeleteBranch { branch: Box<[u8]> },
}
/// A decoded terminal value for one forge call.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// Return items.
    Items { items: Box<[Summary]>, more: bool, now: Time },
    /// Return item.
    Item { item: Summary, comments: Box<[Comment]>, more: bool },
    /// Return pull.
    Pull(Pull),
    /// Return reviews.
    Reviews { reviews: Box<[Review]>, more: bool },
    /// Return statuses.
    Statuses { ci: Ci, statuses: Box<[Status]>, more: bool },
    /// Return remarks.
    Remarks { remarks: Box<[Remark]>, more: bool },
    /// Return commit.
    Commit(Commit),
    /// Return branch names for adoption's deployment-prefix check.
    Branches(Box<[Box<[u8]>]>),
    /// The observed head must equal the request's head.
    PullFiles { head: Commit, files: Box<[File]>, more: bool },
    /// Return compare.
    Compare { before: Commit, after: Commit, contains_before: bool, files: Box<[File]>, commits: Box<[Commit]> },
    /// Return checks.
    Checks(Box<[Status]>),
    /// Bytes from one pinned job attempt, cut to the requested byte budget.
    Job { attempt: JobAttempt, log: Box<[u8]>, truncated: bool },
    /// One bounded file from the requested immutable head.
    File { head: Commit, path: Box<[u8]>, bytes: Box<[u8]>, truncated: bool },
    /// Return protection.
    Protection(Option<Protection>),
    /// Return settings.
    Settings(Settings),
    /// Return collaborators.
    Collaborators { collaborators: Box<[Collaborator]>, more: bool },
    /// Return permission.
    Permission(Permission),
    /// Return created.
    Created(u64),
    /// Return commented.
    Commented(u64),
    /// Return reviewed.
    Reviewed(u64),
    /// Return merged.
    Merged(Commit),
    /// Return branch.
    Branch(BranchCreation),
    /// Return done.
    Done,
}
/// A typed refusal or failure of one forge call.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    /// The forge could not be reached.
    Unavailable,
    /// May have reached the forge: a write is uncertain.
    Timeout,
    /// The forge refused calls until its reset.
    RateLimited { after: Duration },
    /// The writer lacks permission.
    Forbidden,
    /// The requested object is absent.
    Missing,
    /// The requested job attempt is absent.
    MissingJob,
    /// The request or answer exceeds a configured bound.
    TooLarge,
    /// The requested operation has no content.
    Empty,
    /// The provider has no room for this operation.
    Full,
    /// The object already exists.
    Exists,
    /// The pull request has nothing to merge.
    NothingToMerge,
    /// The target is closed.
    Closed,
    /// The required head changed.
    Stale,
    /// The change conflicts with its base.
    Conflict,
    /// Protection refused the write.
    Protected,
    /// Repository settings or transport policy refuses this write.
    Refused,
    /// A decoded answer does not match its operation.
    InvalidAnswer,
    /// Client admission refused, no API call was made.
    Busy,
}
/// The kind of an item in the forge listing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    /// An issue item.
    Issue,
    /// A pull request item.
    Pull,
}
/// Whether a forge item is open or closed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum State {
    /// The item remains open.
    Open,
    /// The item is closed.
    Closed,
}
/// The combined CI state at a commit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ci {
    /// No CI contexts were reported.
    None,
    /// At least one required context is pending.
    Pending,
    /// The reported contexts passed.
    Passed,
    /// At least one reported context failed.
    Failed,
}
/// The state of one check context.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Check {
    /// The context has not reached a verdict.
    Pending,
    /// The context passed.
    Passed,
    /// The context failed.
    Failed,
}
/// A review verdict posted to a pull request.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Verdict {
    /// The review approves the change.
    Approve,
    /// The review requests changes.
    RequestChanges,
    /// The review leaves nonblocking remarks.
    Comment,
}
/// The authenticated writer’s repository permission.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Permission {
    /// The writer has no repository access.
    None,
    /// The writer may read the repository.
    Read,
    /// The writer may write the repository.
    Write,
    /// The writer may administer the repository.
    Admin,
}
/// Whether branch creation made a branch or found one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BranchCreation {
    /// The requested branch was created.
    Created,
    /// The requested branch already existed.
    Exists,
}
/// A bounded item row from a changed-items listing.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Summary {
    pub labels: Box<[Box<[u8]>]>,
    pub number: u64,
    pub kind: Kind,
    pub state: State,
    pub author: u64,
    pub key: Option<Box<[u8]>>,
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
    pub updated: Time,
}
/// Evidence about the observable body/version, not merely its creator.
/// An adapter must use Unknown when its provider shape cannot establish
/// whether a historical creation was edited. Original is never a default
/// inferred from a missing editor field.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Provenance {
    /// The observed body is its original version.
    Original,
    /// The observed body was revised.
    Revised,
    /// The provider cannot establish its origin.
    Unknown,
}
/// A comment with its identity and observable version.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Comment {
    pub id: u64,
    pub provenance: Provenance,
    pub author: u64,
    pub created: Time,
    pub revision: u64,
    pub key: Option<Box<[u8]>>,
    pub body: Box<[u8]>,
}
/// A pull request and the facts needed by a change decision.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Pull {
    pub reviewers: Box<[u64]>,
    pub number: u64,
    pub state: State,
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub commit: Commit,
    pub base_commit: Option<Commit>,
    pub merged: Option<Commit>,
    pub mergeable: bool,
    pub ci: Ci,
}
/// A review with its identity, verdict and observed version.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    pub id: u64,
    pub provenance: Provenance,
    /// Changes when the observable review version changes.
    pub revision: u64,
    pub author: u64,
    pub verdict: Verdict,
    pub commit: Commit,
    pub key: Option<Box<[u8]>>,
    pub body: Box<[u8]>,
    pub at: Time,
    pub official: bool,
}
/// A comment attached to a review and a file position.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Remark {
    pub id: u64,
    pub author: u64,
    pub path: Box<[u8]>,
    pub line: u32,
    pub body: Box<[u8]>,
}
/// A check context with a description, link and optional job attempt.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Status {
    pub author: u64,
    pub at: Time,
    pub context: Box<[u8]>,
    pub check: Check,
    pub description: Box<[u8]>,
    pub url: Box<[u8]>,
    pub job: Option<JobAttempt>,
}
/// A Forgejo Actions job attempt whose failing status belongs to `head`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct JobAttempt {
    pub head: Commit,
    pub run: u64,
    pub job: u64,
    pub attempt: u32,
}
/// A changed file with content on either side when present.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct File {
    pub path: Box<[u8]>,
    /// None means absent, preserving additions and deletions.
    pub before: Option<Box<[u8]>>,
    pub after: Option<Box<[u8]>>,
}
/// A branch protection rule as the writer could observe it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Protection {
    pub branch: Box<[u8]>,
    pub contexts: Box<[Box<[u8]>]>,
    pub approvals: u32,
    pub dismiss_stale: bool,
}
/// Repository merge settings and its default branch.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Settings {
    pub default_branch: Box<[u8]>,
    pub merge: bool,
    pub squash: bool,
    pub rebase: bool,
}
/// A repository collaborator and permission.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Collaborator {
    pub user: u64,
    pub permission: Permission,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Shape {
    Items,
    Item,
    Pull,
    Reviews,
    Statuses,
    Remarks,
    Commit,
    Branches,
    PullFiles,
    Compare,
    Checks,
    Job,
    File,
    Protection,
    Settings,
    Collaborators,
    Permission,
    Created,
    Commented,
    Reviewed,
    Merged,
    Branch,
    Done,
}
fn shape(answer: &Answer) -> Shape {
    match answer {
        Answer::Items { .. } => Shape::Items,
        Answer::Item { .. } => Shape::Item,
        Answer::Pull(_) => Shape::Pull,
        Answer::Reviews { .. } => Shape::Reviews,
        Answer::Statuses { .. } => Shape::Statuses,
        Answer::Remarks { .. } => Shape::Remarks,
        Answer::Commit(_) => Shape::Commit,
        Answer::Branches(_) => Shape::Branches,
        Answer::PullFiles { .. } => Shape::PullFiles,
        Answer::Compare { .. } => Shape::Compare,
        Answer::Checks(_) => Shape::Checks,
        Answer::Job { .. } => Shape::Job,
        Answer::File { .. } => Shape::File,
        Answer::Protection(_) => Shape::Protection,
        Answer::Settings(_) => Shape::Settings,
        Answer::Collaborators { .. } => Shape::Collaborators,
        Answer::Permission(_) => Shape::Permission,
        Answer::Created(_) => Shape::Created,
        Answer::Commented(_) => Shape::Commented,
        Answer::Reviewed(_) => Shape::Reviewed,
        Answer::Merged(_) => Shape::Merged,
        Answer::Branch(_) => Shape::Branch,
        Answer::Done => Shape::Done,
    }
}
pub(crate) fn accepts(op: &Op, answer: &Answer) -> bool {
    let expected = match op {
        Op::Read(read) => match read {
            Read::Items { .. } => Shape::Items,
            Read::Item { .. } => Shape::Item,
            Read::Pull { .. } | Read::PullFor { .. } => Shape::Pull,
            Read::Reviews { .. } => Shape::Reviews,
            Read::Statuses { .. } => Shape::Statuses,
            Read::Remarks { .. } => Shape::Remarks,
            Read::Branch { .. } => Shape::Commit,
            Read::Branches => Shape::Branches,
            Read::PullFiles { .. } => Shape::PullFiles,
            Read::Compare { .. } => Shape::Compare,
            Read::Checks { .. } => Shape::Checks,
            Read::Job { .. } => Shape::Job,
            Read::File { .. } => Shape::File,
            Read::Protection { .. } => Shape::Protection,
            Read::Settings => Shape::Settings,
            Read::Collaborators { .. } => Shape::Collaborators,
            Read::Permission { .. } => Shape::Permission,
        },
        Op::Write(write) => match write {
            Write::CreateIssue { .. } | Write::OpenPull { .. } => Shape::Created,
            Write::Post { .. } => Shape::Commented,
            Write::Review { .. } => Shape::Reviewed,
            Write::Merge { .. } => Shape::Merged,
            Write::CreateBranch { .. } => Shape::Branch,
            Write::Update { .. }
            | Write::Edit { .. }
            | Write::SetReviewers { .. }
            | Write::Close { .. }
            | Write::Reopen { .. }
            | Write::Status { .. }
            | Write::DeleteBranch { .. } => Shape::Done,
        },
    };
    if shape(answer) != expected {
        return false;
    }
    match op {
        Op::Read(read) => match read {
            Read::Items { kind, .. } => items_ordered(answer, *kind),
            Read::Job { attempt, max_bytes } => match answer {
                Answer::Job { attempt: received, log, .. } => {
                    received == attempt && log.len() <= usize::try_from(*max_bytes).expect("u32 fits usize")
                }
                Answer::Items { .. }
                | Answer::Item { .. }
                | Answer::Pull(_)
                | Answer::Reviews { .. }
                | Answer::Statuses { .. }
                | Answer::Remarks { .. }
                | Answer::Commit(_)
                | Answer::Branches(_)
                | Answer::PullFiles { .. }
                | Answer::Compare { .. }
                | Answer::File { .. }
                | Answer::Checks(_)
                | Answer::Protection(_)
                | Answer::Settings(_)
                | Answer::Collaborators { .. }
                | Answer::Permission(_)
                | Answer::Created(_)
                | Answer::Commented(_)
                | Answer::Reviewed(_)
                | Answer::Merged(_)
                | Answer::Branch(_)
                | Answer::Done => false,
            },
            Read::File { head, path, max_bytes } => file_matches(answer, *head, path, *max_bytes),
            Read::Checks { commit } => checks_at_head(answer, *commit),
            Read::PullFiles { head, .. } => observed_head(answer) == Some(*head),
            Read::Compare { before, after } => compared(answer, *before, *after),
            Read::Item { number, after } => item_matches(answer, *number, *after),
            Read::Reviews { .. } => reviews_ordered(answer),
            Read::Pull { number } => match pull_answer(answer) {
                Some(pull) => pull.number == *number,
                None => false,
            },
            Read::PullFor { head, base } => match pull_answer(answer) {
                Some(pull) => pull.head == *head && pull.base == *base,
                None => false,
            },
            Read::Statuses { .. }
            | Read::Remarks { .. }
            | Read::Branch { .. }
            | Read::Branches
            | Read::Protection { .. }
            | Read::Settings
            | Read::Collaborators { .. }
            | Read::Permission { .. } => true,
        },
        Op::Write(_) => true,
    }
}
fn checks_at_head(answer: &Answer, head: Commit) -> bool {
    match answer {
        Answer::Checks(statuses) => {
            for status in statuses {
                match status.job {
                    Some(job) if job.head != head => return false,
                    Some(_) | None => {}
                }
            }
            true
        }
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}
fn items_ordered(answer: &Answer, kind: Option<Kind>) -> bool {
    match answer {
        Answer::Items { items, .. } => {
            let mut last_time = Time::ZERO;
            let mut last_number = 0;
            for (index, item) in items.iter().enumerate() {
                if item.updated < last_time
                    || (item.updated == last_time && item.number <= last_number)
                    || item.number == 0
                    || (kind.is_some() && kind != Some(item.kind))
                {
                    return false;
                }
                for previous in items.get(..index).expect("enumerated item index") {
                    if previous.number == item.number {
                        return false;
                    }
                }
                last_time = item.updated;
                last_number = item.number;
            }
            true
        }
        Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}
fn reviews_ordered(answer: &Answer) -> bool {
    match answer {
        Answer::Reviews { reviews, .. } => {
            let mut last = 0;
            for review in reviews {
                if review.id <= last {
                    return false;
                }
                last = review.id;
            }
            true
        }
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}
fn item_matches(answer: &Answer, number: u64, after: u64) -> bool {
    match answer {
        Answer::Item { item, comments, .. } => {
            if item.number != number {
                return false;
            }
            let mut last = after;
            for comment in comments {
                if comment.id <= last {
                    return false;
                }
                last = comment.id;
            }
            true
        }
        Answer::Items { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}
fn pull_answer(answer: &Answer) -> Option<&Pull> {
    match answer {
        Answer::Pull(pull) => Some(pull),
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => None,
    }
}
fn observed_head(answer: &Answer) -> Option<Commit> {
    match answer {
        Answer::PullFiles { head, .. } => Some(*head),
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => None,
    }
}
fn compared(answer: &Answer, before: Commit, after: Commit) -> bool {
    match answer {
        Answer::Compare { before: observed_before, after: observed_after, .. } => {
            *observed_before == before && *observed_after == after
        }
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}

fn file_matches(answer: &Answer, head: Commit, path: &[u8], max_bytes: u32) -> bool {
    match answer {
        Answer::File { head: received, path: received_path, bytes, .. } => {
            *received == head
                && received_path.as_ref() == path
                && bytes.len() <= usize::try_from(max_bytes).expect("u32 fits usize")
        }
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}
