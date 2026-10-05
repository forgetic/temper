//! The translations between the host's vocabulary and its siblings' (4.5):
//! siblings share no types, so the host's workspaces meet the checkout's
//! specs and outcomes here, and its agents the agent child domain's channel,
//! through small total functions, each an exhaustive match, so that a variant
//! added on either side breaks the build in one place.

use alloc::boxed::Box;

use skein_lib::List;
use skein_lib::bytes::copy_of;
use temper_worker_domain_agent::{self as agent, channel};
use temper_worker_domain_checkout::{self as checkout, git};
use temper_worker_domain_host as host;

use crate::boundary::Phase;

/// The title of the commit that saves a run's unfinished work.
pub(crate) const SAVED: &[u8] = b"Save unfinished work";

/// The checkout's spec for the host's workspace: the same repositories, in
/// the same order, each reached as its identity.
pub(crate) fn spec(workspace: host::Workspace) -> checkout::Spec {
    let host::Workspace { key, repositories } = workspace;
    let count = u32::try_from(repositories.len()).expect("the host checked the repositories against its limits");
    let mut specs = List::with_capacity(count);
    for repository in repositories {
        specs.push(self::repository(repository)).expect("room for every repository");
    }
    checkout::Spec { key, repositories: specs.into_boxed() }
}

#[expect(clippy::manual_map, reason = "explicit option match follows the step subset, without function pointers")]
fn repository(repository: host::Repository) -> checkout::Repository {
    let host::Repository { tag: _, name, remote, start, access, identity } = repository;
    let (push, expected) = match access {
        host::Access::ReadOnly => (None, None),
        host::Access::Writable { push } => (Some(push), None),
        host::Access::WritableV2 { push, expected } => {
            let expected = match expected {
                Some(raw) => Some(git::Commit::new(raw)),
                None => None,
            };
            (Some(push), expected)
        }
    };
    checkout::Repository { name, remote, start: self::start(start), identity, push, expected }
}

fn start(start: host::Start) -> checkout::Start {
    match start {
        host::Start::Base { branch } => checkout::Start::Base { branch },
        host::Start::Branch { branch } => checkout::Start::Branch { branch },
        host::Start::Commit { commit } => checkout::Start::Commit { commit: git::Commit::new(commit) },
        host::Start::Saved { branch } => checkout::Start::Saved { branch },
        host::Start::Merge { branch, base } => checkout::Start::Merge { branch, base: git::Commit::new(base) },
    }
}

/// Why a prepare failed, as the host tells the engine: a retry may work, or
/// what the forge lacks or refused, for which repository.
pub(crate) const fn failure(failure: checkout::Failure) -> host::Preparation {
    match failure {
        checkout::Failure::Transient => host::Preparation::Transient,
        checkout::Failure::Missing { repository, missing } => {
            host::Preparation::Missing { repository, missing: self::missing(missing) }
        }
        checkout::Failure::Refused { repository } => host::Preparation::Refused { repository },
    }
}

const fn missing(missing: git::Missing) -> host::Missing {
    match missing {
        git::Missing::Repository => host::Missing::Repository,
        git::Missing::Branch => host::Missing::Branch,
        git::Missing::Commit => host::Missing::Commit,
    }
}

/// Why a prepare was refused at the checkout's entrance, as the host tells the
/// engine: a workstream whose workspace another run holds, or a disk with
/// every workspace held, may have room later. The host admits no workspace
/// the checkout cannot take, as `worst_case` checks their limits agree.
pub(crate) fn refusal(refusal: checkout::Refusal) -> host::Preparation {
    match refusal {
        checkout::Refusal::Busy | checkout::Refusal::Full => host::Preparation::Transient,
        checkout::Refusal::Invalid => unreachable!("the host admits only workspaces the checkout takes"),
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
pub(crate) fn landings(outcome: checkout::Outcome, repositories: u32) -> Box<[host::Landing]> {
    let mut landings = List::with_capacity(repositories);
    match outcome {
        checkout::Outcome::Pushed { landings: pushed } => {
            for landed in pushed {
                landings.push(landing(landed)).expect("one landing for each repository");
            }
        }
        checkout::Outcome::Refused { refusal: _ } => {
            for _ in 0..repositories {
                landings.push(host::Landing::Failed).expect("one landing for each repository");
            }
        }
    }
    landings.into_boxed()
}

fn landing(landing: checkout::Landing) -> host::Landing {
    match landing {
        checkout::Landing::Conflicted { files } => host::Landing::Conflicted { files },
        checkout::Landing::Explained { fault, diagnostic } => host::Landing::Explained {
            failure: host::PushFailure {
                repository: None,
                reason: push_reason(fault),
                diagnostic: host::PushDiagnostic::new(diagnostic.output(), diagnostic.cut()),
            },
        },
        checkout::Landing::Landed { commit } => host::Landing::Landed { commit: commit.raw() },
        checkout::Landing::Moved => host::Landing::Moved,
        checkout::Landing::Unchanged => host::Landing::Unchanged,
        checkout::Landing::Refused => host::Landing::Refused,
        checkout::Landing::Failed | checkout::Landing::Aborted => host::Landing::Failed,
    }
}

pub(crate) fn ask(ask: channel::Ask) -> host::Ask {
    match ask {
        channel::Ask::PushV2 { title, body } => host::Ask::PushV2 { title, body },
        channel::Ask::Push { message } => host::Ask::Push { message },
        channel::Ask::Relay { body } => host::Ask::Relay { body },
    }
}

pub(crate) fn reply(reply: host::Reply) -> channel::Reply {
    match reply {
        host::Reply::Relayed { answer } => channel::Reply::Relayed { answer },
        host::Reply::Pushed(push) => channel::Reply::Pushed(self::push(push)),
        host::Reply::Unavailable => channel::Reply::Unavailable,
        host::Reply::Withdrawn => channel::Reply::Withdrawn,
        host::Reply::Busy => channel::Reply::Busy,
    }
}

fn push(push: host::Push) -> channel::Push {
    match push {
        host::Push::Conflicted { repository, files } => channel::Push::Conflicted { repository, files },
        host::Push::Done => channel::Push::Done,
        host::Push::Moved => channel::Push::Moved,
        host::Push::Failed { failure } => channel::Push::Failed { failure: channel_failure(&failure) },
        host::Push::Nothing => channel::Push::Nothing,
    }
}

pub(crate) fn finish(finish: channel::Finish) -> host::Finish {
    match finish {
        channel::Finish::Ended { outcome } => host::Finish::Ended { outcome },
        channel::Finish::Parked { snapshot } => host::Finish::Parked { snapshot },
        channel::Finish::Failed { failure } => host::Finish::Failed { failure: run_failure(failure) },
    }
}

const fn run_failure(failure: channel::RunFailure) -> host::RunFailure {
    match failure {
        channel::RunFailure::Model => host::RunFailure::Model,
        channel::RunFailure::Budget => host::RunFailure::Budget,
        channel::RunFailure::Policy => host::RunFailure::Policy,
        channel::RunFailure::Cancelled => host::RunFailure::Cancelled,
        channel::RunFailure::Stale => host::RunFailure::Stale,
        channel::RunFailure::Exhausted => host::RunFailure::Exhausted,
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

pub(crate) const fn bounce(bounce: agent::Bounce) -> host::Bounce {
    match bounce {
        agent::Bounce::TooLarge => host::Bounce::TooLarge,
        agent::Bounce::Full => host::Bounce::Full,
        agent::Bounce::Ending => host::Bounce::Ending,
    }
}

pub(crate) const fn phase(phase: host::Phase) -> Phase {
    match phase {
        host::Phase::Preparing => Phase::Preparing,
        host::Phase::Starting => Phase::Starting,
        host::Phase::Active => Phase::Active,
        host::Phase::Waiting => Phase::Waiting,
        host::Phase::Ending => Phase::Ending,
    }
}

const fn push_reason(fault: git::Fault) -> host::PushReason {
    match fault {
        git::Fault::Missing { missing: git::Missing::Repository } => host::PushReason::MissingRepository,
        git::Fault::Missing { missing: git::Missing::Branch } => host::PushReason::MissingBranch,
        git::Fault::Missing { missing: git::Missing::Commit } => host::PushReason::MissingCommit,
        git::Fault::Refused => host::PushReason::Refused,
        git::Fault::Unreachable => host::PushReason::Unreachable,
        git::Fault::Broken => host::PushReason::Broken,
        git::Fault::TimedOut => host::PushReason::TimedOut,
        git::Fault::Cancelled => host::PushReason::Cancelled,
    }
}

fn channel_failure(failure: &host::PushFailure) -> agent::PushFailure {
    let host::PushFailure { repository, reason, diagnostic } = failure;
    let reason = match reason {
        host::PushReason::MissingRepository => agent::PushReason::MissingRepository,
        host::PushReason::MissingBranch => agent::PushReason::MissingBranch,
        host::PushReason::MissingCommit => agent::PushReason::MissingCommit,
        host::PushReason::Refused => agent::PushReason::Refused,
        host::PushReason::Unreachable => agent::PushReason::Unreachable,
        host::PushReason::Broken => agent::PushReason::Broken,
        host::PushReason::TimedOut => agent::PushReason::TimedOut,
        host::PushReason::Cancelled => agent::PushReason::Cancelled,
        host::PushReason::Unavailable => agent::PushReason::Unavailable,
        host::PushReason::Busy => agent::PushReason::Busy,
        host::PushReason::TooLarge => agent::PushReason::TooLarge,
        host::PushReason::Nothing => agent::PushReason::Nothing,
        host::PushReason::Unknown => agent::PushReason::Unknown,
    };
    agent::PushFailure {
        repository: *repository,
        reason,
        diagnostic: agent::PushDiagnostic::new(diagnostic.output(), diagnostic.cut()),
    }
}

pub(crate) fn finish_v2(finish: channel::FinishV2) -> host::FinishV2 {
    match finish {
        channel::FinishV2::Ended { outcome } => host::FinishV2::Ended { outcome },
        channel::FinishV2::Parked => host::FinishV2::Parked,
        channel::FinishV2::Failed { failure } => host::FinishV2::Failed { failure: run_failure(failure) },
    }
}
