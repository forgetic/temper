//! The translations between the host's vocabulary and its siblings' (4.5):
//! siblings share no types, so the host's workspaces meet the checkout's
//! specs and outcomes here, and its agents the agent child domain's channel,
//! through small total functions, each an exhaustive match, so that a variant
//! added on either side breaks the build in one place.

use alloc::boxed::Box;

use crate::wire;
use jig_host as host;
use skein_lib::bytes::copy_of;
use skein_lib::{List, Token};
use temper_worker_domain_agent::{self as agent, channel};
use temper_worker_domain_checkout::{self as checkout, git};

use crate::boundary::Phase;
use crate::domain::Domain;
use crate::workspace;

/// The title of the commit that saves a run's unfinished work.
pub(crate) const SAVED: &[u8] = b"Save unfinished work";

/// The checkout's spec for the host's workspace: the same repositories, in
/// the same order, each reached as its identity.
pub(crate) fn spec(workspace: wire::Workspace) -> checkout::Spec {
    let wire::Workspace { key, repositories } = workspace;
    let count = u32::try_from(repositories.len()).expect("the host checked the repositories against its limits");
    let mut specs = List::with_capacity(count);
    for repository in repositories {
        specs.push(self::repository(repository)).expect("room for every repository");
    }
    checkout::Spec { key, repositories: specs.into_boxed() }
}

#[expect(clippy::manual_map, reason = "explicit option match follows the step subset, without function pointers")]
fn repository(repository: wire::Repository) -> checkout::Repository {
    let wire::Repository { tag: _, name, remote, start, access, identity } = repository;
    let (push, expected) = match access {
        wire::Access::ReadOnly => (None, None),
        wire::Access::Writable { push } => (Some(push), None),
        wire::Access::WritableV2 { push, expected } => {
            let expected = match expected {
                Some(raw) => Some(git::Commit::new(raw)),
                None => None,
            };
            (Some(push), expected)
        }
    };
    checkout::Repository { name, remote, start: self::start(start), identity, push, expected }
}

fn start(start: wire::Start) -> checkout::Start {
    match start {
        wire::Start::Base { branch } => checkout::Start::Base { branch },
        wire::Start::Branch { branch } => checkout::Start::Branch { branch },
        wire::Start::Commit { commit } => checkout::Start::Commit { commit: git::Commit::new(commit) },
        wire::Start::Saved { branch } => checkout::Start::Saved { branch },
        wire::Start::Merge { branch, base } => checkout::Start::Merge { branch, base: git::Commit::new(base) },
    }
}

/// Why a prepare failed, as the host tells the engine: a retry may work, or
/// what the forge lacks or refused, for which repository.
pub(crate) const fn failure(failure: checkout::Failure) -> wire::Preparation {
    match failure {
        checkout::Failure::Transient => wire::Preparation::Transient,
        checkout::Failure::Missing { repository, missing } => {
            wire::Preparation::Missing { repository, missing: self::missing(missing) }
        }
        checkout::Failure::Refused { repository } => wire::Preparation::Refused { repository },
    }
}

const fn missing(missing: git::Missing) -> wire::Missing {
    match missing {
        git::Missing::Repository => wire::Missing::Repository,
        git::Missing::Branch => wire::Missing::Branch,
        git::Missing::Commit => wire::Missing::Commit,
    }
}

/// Why a prepare was refused at the checkout's entrance, as the host tells the
/// engine: a workstream whose workspace another run holds, or a disk with
/// every workspace held, may have room later. The host admits no workspace
/// the checkout cannot take, as `worst_case` checks their limits agree.
pub(crate) fn refusal(refusal: checkout::Refusal) -> wire::Preparation {
    match refusal {
        checkout::Refusal::Busy | checkout::Refusal::Full => wire::Preparation::Transient,
        checkout::Refusal::Invalid => unreachable!("the host admits only workspaces the checkout takes"),
    }
}

pub(crate) const fn preparation(failure: wire::Preparation, resource: Token) -> host::Preparation {
    match failure {
        wire::Preparation::Transient => host::Preparation::Transient,
        wire::Preparation::Missing { .. } | wire::Preparation::Refused { .. } => {
            host::Preparation::Permanent { resource: Some(resource) }
        }
    }
}

/// The commit message of a run's push: what the run said, as its title.
pub(crate) fn message(message: Box<[u8]>) -> checkout::Message {
    checkout::Message { title: message, body: Box::new([]) }
}

/// The commit message of a save.
pub(crate) fn saved() -> checkout::Message {
    checkout::Message { title: copy_of(SAVED), body: Box::new([]) }
}

/// What became of each of the workspace's `repositories` in a push or a save,
/// as the host hears it. One refused at the checkout's entrance pushed
/// nothing, so failed everywhere; a repository the push did not reach, as it
/// was aborted, failed.
pub(crate) fn landings(outcome: checkout::Outcome, repositories: u32) -> Box<[wire::Landing]> {
    let mut landings = List::with_capacity(repositories);
    match outcome {
        checkout::Outcome::Pushed { landings: pushed } => {
            for landed in pushed {
                landings.push(landing(landed)).expect("one landing for each repository");
            }
        }
        checkout::Outcome::Refused { refusal: _ } => {
            for _ in 0..repositories {
                landings.push(wire::Landing::Failed).expect("one landing for each repository");
            }
        }
    }
    landings.into_boxed()
}

fn landing(landing: checkout::Landing) -> wire::Landing {
    match landing {
        checkout::Landing::Conflicted { files } => wire::Landing::Conflicted { files },
        checkout::Landing::Explained { fault, diagnostic } => wire::Landing::Explained {
            failure: wire::PushFailure {
                repository: None,
                reason: push_reason(fault),
                diagnostic: wire::PushDiagnostic::new(diagnostic.output(), diagnostic.cut()),
            },
        },
        checkout::Landing::Landed { commit } => wire::Landing::Landed { commit: commit.raw() },
        checkout::Landing::Moved => wire::Landing::Moved,
        checkout::Landing::Unchanged => wire::Landing::Unchanged,
        checkout::Landing::Refused => wire::Landing::Refused,
        checkout::Landing::Failed | checkout::Landing::Aborted => wire::Landing::Failed,
    }
}

pub(crate) fn ask(ask: channel::Ask) -> host::Ask {
    match ask {
        channel::Ask::PushV2 { title, body } => host::Ask::DeliverV2 { title, body },
        channel::Ask::Push { message } => host::Ask::Deliver { message },
        channel::Ask::Relay { body } => host::Ask::Relay { body },
    }
}

pub(crate) const fn delivery_outcome(result: &wire::Push) -> host::DeliveryOutcome {
    match result {
        wire::Push::Done => host::DeliveryOutcome::Delivered,
        wire::Push::Nothing => host::DeliveryOutcome::Nothing,
        wire::Push::Moved => host::DeliveryOutcome::Stale,
        wire::Push::Conflicted { .. } => host::DeliveryOutcome::Refused,
        wire::Push::Failed { .. } => host::DeliveryOutcome::Failed,
    }
}

pub(crate) fn reply(domain: &Domain, reply: host::Reply) -> channel::Reply {
    match reply {
        host::Reply::Relayed { answer } => channel::Reply::Relayed { answer },
        host::Reply::Delivered(delivery) => {
            channel::Reply::Pushed(push(workspace::delivery_reply(domain, delivery.left)))
        }
        host::Reply::Unavailable => channel::Reply::Unavailable,
        host::Reply::Withdrawn => channel::Reply::Withdrawn,
        host::Reply::Busy => channel::Reply::Busy,
    }
}

pub(crate) fn answer(answer: host::Answer, work: wire::Work, preparation: Option<wire::Preparation>) -> wire::Answer {
    match answer {
        host::Answer::Refused(refusal) => wire::Answer::Refused(host_refusal(refusal)),
        host::Answer::Ended { outcome, work: _ } => wire::Answer::Ended { outcome, work },
        host::Answer::Parked { snapshot, work: _ } => wire::Answer::Parked { snapshot, work },
        host::Answer::Failed { failure, detail, work: _ } => {
            wire::Answer::Failed { failure: host_failure(failure, preparation), detail, work }
        }
    }
}

pub(crate) fn answer_v2(
    answer: host::AnswerV2,
    work: wire::Work,
    preparation: Option<wire::Preparation>,
) -> wire::AnswerV2 {
    let ending = match answer.ending {
        host::EndingV2::Refused(refusal) => wire::EndingV2::Refused(host_refusal(refusal)),
        host::EndingV2::Ended { outcome, work: _ } => wire::EndingV2::Ended { outcome, work },
        host::EndingV2::Parked { work: _ } => wire::EndingV2::Parked { work },
        host::EndingV2::Failed { failure, detail, work: _ } => {
            wire::EndingV2::Failed { failure: host_failure(failure, preparation), detail, work }
        }
    };
    wire::AnswerV2 { turns: answer.turns, spent: answer.spent, ending }
}

const fn host_refusal(refusal: host::Refusal) -> wire::Refusal {
    match refusal {
        host::Refusal::Busy => wire::Refusal::Busy,
        host::Refusal::Invalid(invalid) => wire::Refusal::Invalid(match invalid {
            host::Invalid::Charter => wire::Invalid::Charter,
            host::Invalid::Snapshot => wire::Invalid::Snapshot,
            host::Invalid::Transcript => wire::Invalid::Transcript,
            host::Invalid::Version => wire::Invalid::Version,
            host::Invalid::Grants => wire::Invalid::Grants,
        }),
    }
}

const fn host_failure(failure: host::Failure, preparation: Option<wire::Preparation>) -> wire::Failure {
    match failure {
        host::Failure::Unprepared(_) => wire::Failure::Unprepared(match preparation {
            Some(failure) => failure,
            None => wire::Preparation::Transient,
        }),
        host::Failure::Run(failure) => wire::Failure::Run(failure),
        host::Failure::Agent(failure) => wire::Failure::Agent(failure),
        host::Failure::Cancelled(reason) => wire::Failure::Cancelled(reason),
    }
}

fn push(push: wire::Push) -> channel::Push {
    match push {
        wire::Push::Conflicted { repository, files } => channel::Push::Conflicted { repository, files },
        wire::Push::Done => channel::Push::Done,
        wire::Push::Moved => channel::Push::Moved,
        wire::Push::Failed { failure } => channel::Push::Failed { failure: channel_failure(&failure) },
        wire::Push::Nothing => channel::Push::Nothing,
    }
}

pub(crate) fn finish(finish: channel::Finish) -> wire::Finish {
    match finish {
        channel::Finish::Ended { outcome } => wire::Finish::Ended { outcome },
        channel::Finish::Parked { snapshot } => wire::Finish::Parked { snapshot },
        channel::Finish::Failed { failure } => wire::Finish::Failed { failure: run_failure(failure) },
    }
}

const fn run_failure(failure: channel::RunFailure) -> wire::RunFailure {
    match failure {
        channel::RunFailure::Model => wire::RunFailure::Model,
        channel::RunFailure::Budget => wire::RunFailure::Budget,
        channel::RunFailure::Policy => wire::RunFailure::Policy,
        channel::RunFailure::Cancelled => wire::RunFailure::Cancelled,
        channel::RunFailure::Stale => wire::RunFailure::Stale,
        channel::RunFailure::Exhausted => wire::RunFailure::Exhausted,
    }
}

pub(crate) const fn fault(fault: agent::Fault) -> host::AgentFailure {
    match fault {
        agent::Fault::Exited => host::AgentFailure::Exited,
        agent::Fault::Rules => host::AgentFailure::Rules,
        agent::Fault::NoProgress => host::AgentFailure::NoProgress,
        agent::Fault::WallTime => host::AgentFailure::WallTime,
    }
}

pub(crate) const fn bounce(bounce: agent::Bounce) -> wire::Bounce {
    match bounce {
        agent::Bounce::TooLarge => wire::Bounce::TooLarge,
        agent::Bounce::Full => wire::Bounce::Full,
        agent::Bounce::Ending => wire::Bounce::Ending,
    }
}

pub(crate) const fn phase(phase: wire::Phase) -> Phase {
    match phase {
        wire::Phase::Preparing => Phase::Preparing,
        wire::Phase::Starting => Phase::Starting,
        wire::Phase::Active => Phase::Active,
        wire::Phase::Waiting => Phase::Waiting,
        wire::Phase::Ending => Phase::Ending,
    }
}

const fn push_reason(fault: git::Fault) -> wire::PushReason {
    match fault {
        git::Fault::Missing { missing: git::Missing::Repository } => wire::PushReason::MissingRepository,
        git::Fault::Missing { missing: git::Missing::Branch } => wire::PushReason::MissingBranch,
        git::Fault::Missing { missing: git::Missing::Commit } => wire::PushReason::MissingCommit,
        git::Fault::Refused => wire::PushReason::Refused,
        git::Fault::Unreachable => wire::PushReason::Unreachable,
        git::Fault::Broken => wire::PushReason::Broken,
        git::Fault::TimedOut => wire::PushReason::TimedOut,
        git::Fault::Cancelled => wire::PushReason::Cancelled,
    }
}

fn channel_failure(failure: &wire::PushFailure) -> agent::PushFailure {
    let wire::PushFailure { repository, reason, diagnostic } = failure;
    let reason = match reason {
        wire::PushReason::MissingRepository => agent::PushReason::MissingRepository,
        wire::PushReason::MissingBranch => agent::PushReason::MissingBranch,
        wire::PushReason::MissingCommit => agent::PushReason::MissingCommit,
        wire::PushReason::Refused => agent::PushReason::Refused,
        wire::PushReason::Unreachable => agent::PushReason::Unreachable,
        wire::PushReason::Broken => agent::PushReason::Broken,
        wire::PushReason::TimedOut => agent::PushReason::TimedOut,
        wire::PushReason::Cancelled => agent::PushReason::Cancelled,
        wire::PushReason::Unavailable => agent::PushReason::Unavailable,
        wire::PushReason::Busy => agent::PushReason::Busy,
        wire::PushReason::TooLarge => agent::PushReason::TooLarge,
        wire::PushReason::Nothing => agent::PushReason::Nothing,
        wire::PushReason::Unknown => agent::PushReason::Unknown,
    };
    agent::PushFailure {
        repository: *repository,
        reason,
        diagnostic: agent::PushDiagnostic::new(diagnostic.output(), diagnostic.cut()),
    }
}

pub(crate) fn finish_v2(finish: channel::FinishV2) -> wire::FinishV2 {
    match finish {
        channel::FinishV2::Ended { outcome } => wire::FinishV2::Ended { outcome },
        channel::FinishV2::Parked => wire::FinishV2::Parked,
        channel::FinishV2::Failed { failure } => wire::FinishV2::Failed { failure: run_failure(failure) },
    }
}

/// What the run is told of a push: done only if every repository with a
/// change landed it; moved if any branch moved; failed if the forge refused
/// one, or one failed.
pub(crate) fn summarize(push: Box<[wire::Landing]>) -> wire::Push {
    let mut landed = false;
    let mut moved = false;
    let mut first = None;
    let mut index = 0_u32;
    for landing in push {
        let failed = match landing {
            wire::Landing::Conflicted { files } => Some(wire::Push::Conflicted { repository: index, files }),
            wire::Landing::Landed { .. } => {
                landed = true;
                None
            }
            wire::Landing::Moved => {
                moved = true;
                None
            }
            wire::Landing::Failed => Some(wire::Push::Failed {
                failure: wire::PushFailure {
                    repository: Some(index),
                    reason: wire::PushReason::Unknown,
                    diagnostic: wire::PushDiagnostic::empty(),
                },
            }),
            wire::Landing::Refused => Some(wire::Push::Failed {
                failure: wire::PushFailure {
                    repository: Some(index),
                    reason: wire::PushReason::Refused,
                    diagnostic: wire::PushDiagnostic::empty(),
                },
            }),
            wire::Landing::Explained { mut failure } => {
                failure.repository = Some(index);
                Some(wire::Push::Failed { failure })
            }
            wire::Landing::Unchanged => None,
        };
        if first.is_none() {
            first = failed;
        }
        index = index.checked_add(1).expect("bounded repository count");
    }
    if moved {
        wire::Push::Moved
    } else if let Some(failed) = first {
        failed
    } else if landed {
        wire::Push::Done
    } else {
        wire::Push::Nothing
    }
}
