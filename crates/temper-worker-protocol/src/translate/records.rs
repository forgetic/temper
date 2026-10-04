//! Field-for-field records shared by link and agent conversions.
use super::Error;
use skein_lib::{List, bytes::copy_of};
use temper_channel::wire;
use temper_worker_domain::{agent::channel, host};
pub(super) fn run_to(value: host::RunFailure) -> wire::RunFailure {
    match value {
        host::RunFailure::Model => wire::RunFailure::Model,
        host::RunFailure::Budget => wire::RunFailure::Budget,
        host::RunFailure::Policy => wire::RunFailure::Policy,
        host::RunFailure::Cancelled => wire::RunFailure::Cancelled,
        host::RunFailure::Stale => wire::RunFailure::Stale,
        host::RunFailure::Exhausted => wire::RunFailure::Exhausted,
    }
}
pub(super) fn run_from(value: wire::RunFailure) -> channel::RunFailure {
    match value {
        wire::RunFailure::Model => channel::RunFailure::Model,
        wire::RunFailure::Budget => channel::RunFailure::Budget,
        wire::RunFailure::Policy => channel::RunFailure::Policy,
        wire::RunFailure::Cancelled => channel::RunFailure::Cancelled,
        wire::RunFailure::Stale => channel::RunFailure::Stale,
        wire::RunFailure::Exhausted => channel::RunFailure::Exhausted,
    }
}
pub(super) fn agent_to(value: host::AgentFailure) -> wire::AgentFailure {
    match value {
        host::AgentFailure::Unstarted => wire::AgentFailure::Unstarted,
        host::AgentFailure::Exited => wire::AgentFailure::Exited,
        host::AgentFailure::Rules => wire::AgentFailure::Rules,
        host::AgentFailure::NoProgress => wire::AgentFailure::NoProgress,
        host::AgentFailure::WallTime => wire::AgentFailure::WallTime,
    }
}
pub(super) fn reason_to(value: host::Reason) -> wire::CancelReason {
    match value {
        host::Reason::Engine => wire::CancelReason::Engine,
        host::Reason::Contact => wire::CancelReason::Contact,
        host::Reason::Shutdown => wire::CancelReason::Shutdown,
    }
}
pub(super) fn missing_to(value: host::Missing) -> wire::Missing {
    match value {
        host::Missing::Repository => wire::Missing::Repository,
        host::Missing::Branch => wire::Missing::Branch,
        host::Missing::Commit => wire::Missing::Commit,
    }
}
pub(super) fn phase_to(value: temper_worker_domain::Phase) -> wire::HostingPhase {
    match value {
        temper_worker_domain::Phase::Preparing => wire::HostingPhase::Preparing,
        temper_worker_domain::Phase::Starting => wire::HostingPhase::Starting,
        temper_worker_domain::Phase::Active => wire::HostingPhase::Active,
        temper_worker_domain::Phase::Waiting => wire::HostingPhase::Waiting,
        temper_worker_domain::Phase::Ending => wire::HostingPhase::Ending,
        temper_worker_domain::Phase::Answered => wire::HostingPhase::Answered,
    }
}
pub(super) fn bounce_to(value: host::Bounce) -> wire::Bounce {
    match value {
        host::Bounce::TooLarge => wire::Bounce::TooLarge,
        host::Bounce::Full => wire::Bounce::Full,
        host::Bounce::Ending => wire::Bounce::Ending,
    }
}
pub(super) fn push_reason_to(value: channel::PushReason) -> wire::PushReason {
    match value {
        channel::PushReason::MissingRepository => wire::PushReason::MissingRepository,
        channel::PushReason::MissingBranch => wire::PushReason::MissingBranch,
        channel::PushReason::MissingCommit => wire::PushReason::MissingCommit,
        channel::PushReason::Refused => wire::PushReason::Refused,
        channel::PushReason::Unreachable => wire::PushReason::Unreachable,
        channel::PushReason::Broken => wire::PushReason::Broken,
        channel::PushReason::TimedOut => wire::PushReason::TimedOut,
        channel::PushReason::Cancelled => wire::PushReason::Cancelled,
        channel::PushReason::Unavailable => wire::PushReason::Unavailable,
        channel::PushReason::Busy => wire::PushReason::Busy,
        channel::PushReason::TooLarge => wire::PushReason::TooLarge,
        channel::PushReason::Nothing => wire::PushReason::Nothing,
        channel::PushReason::Unknown => wire::PushReason::Unknown,
    }
}

pub(super) fn start_from(value: wire::Start) -> host::Start {
    match value {
        wire::Start::Base { branch } => host::Start::Base { branch },
        wire::Start::Branch { branch } => host::Start::Branch { branch },
        wire::Start::Commit { commit } => host::Start::Commit { commit },
        wire::Start::Saved { branch } => host::Start::Saved { branch },
    }
}
pub(super) fn access_from(value: wire::Access) -> host::Access {
    match value {
        wire::Access::ReadOnly => host::Access::ReadOnly,
        wire::Access::Writable { push } => host::Access::Writable { push },
    }
}
pub(super) fn preparation_to(value: host::Preparation) -> wire::Preparation {
    match value {
        host::Preparation::Transient => wire::Preparation::Transient,
        host::Preparation::Missing { repository, missing } => {
            wire::Preparation::Missing { repository, missing: missing_to(missing) }
        }
        host::Preparation::Refused { repository } => wire::Preparation::Refused { repository },
    }
}
pub(super) fn failure_to(value: host::Failure) -> wire::Failure {
    match value {
        host::Failure::Unprepared(preparation) => {
            wire::Failure::Unprepared { preparation: preparation_to(preparation) }
        }
        host::Failure::Run(failure) => wire::Failure::Run { failure: run_to(failure) },
        host::Failure::Agent(failure) => wire::Failure::Agent { failure: agent_to(failure) },
        host::Failure::Cancelled(reason) => wire::Failure::Cancelled { reason: reason_to(reason) },
    }
}
pub(super) fn invalid_to(value: host::Invalid) -> Result<wire::Invalid, Error> {
    Ok(match value {
        host::Invalid::Repositories => wire::Invalid::Repositories,
        host::Invalid::Duplicate => wire::Invalid::Duplicate,
        host::Invalid::Name => wire::Invalid::Name,
        host::Invalid::Charter => wire::Invalid::Charter,
        host::Invalid::Snapshot => wire::Invalid::Snapshot,
        host::Invalid::Grants => return Err(Error::Unsupported),
    })
}
pub(super) fn push_failure_to(value: &channel::PushFailure) -> wire::PushFailure {
    wire::PushFailure {
        repository: value.repository,
        reason: push_reason_to(value.reason),
        output: copy_of(value.diagnostic.output()),
        cut: value.diagnostic.cut(),
    }
}
pub(super) fn landing_to(value: &host::Landing) -> wire::Landing {
    match value {
        host::Landing::Explained { failure } => wire::Landing::Explained { failure: host_push_failure_to(failure) },
        host::Landing::Landed { commit } => wire::Landing::Landed { commit: *commit },
        host::Landing::Moved => wire::Landing::Moved,
        host::Landing::Failed => wire::Landing::Failed,
        host::Landing::Refused => wire::Landing::Refused,
        host::Landing::Unchanged => wire::Landing::Unchanged,
    }
}
pub(super) fn work_to(value: host::Work, sizes: &temper_channel::Sizes) -> Result<wire::Work, Error> {
    let saved_count = match &value.saved {
        Some(saved) => saved.len(),
        None => 0,
    };
    if u32::try_from(value.landed.len()).ok().ok_or(Error::Limits)? > sizes.repositories
        || u32::try_from(saved_count).ok().ok_or(Error::Limits)? > sizes.repositories
    {
        return Err(Error::Limits);
    }
    let mut landed = List::with_capacity(u32::try_from(value.landed.len()).ok().ok_or(Error::Limits)?);
    for record in value.landed {
        landed.push(wire::Landed { tag: record.tag, commit: record.commit }).expect("source landing count");
    }
    let saved = match value.saved {
        Some(source) => {
            let mut saved = List::with_capacity(u32::try_from(source.len()).ok().ok_or(Error::Limits)?);
            for record in source {
                saved.push(landing_to(&record)).expect("source saved count");
            }
            Some(saved.into_boxed())
        }
        None => None,
    };
    Ok(wire::Work { landed: landed.into_boxed(), saved })
}
pub(super) fn answer_to(value: host::Answer, sizes: &temper_channel::Sizes) -> Result<wire::LinkAnswer, Error> {
    Ok(match value {
        host::Answer::Refused(host::Refusal::Busy) => {
            wire::LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy }
        }
        host::Answer::Refused(host::Refusal::Invalid(invalid)) => {
            wire::LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Invalid { invalid: invalid_to(invalid)? } }
        }
        host::Answer::Ended { outcome, work } => wire::LinkAnswer::Ended { outcome, work: work_to(work, sizes)? },
        host::Answer::Parked { snapshot, work } => wire::LinkAnswer::Parked { snapshot, work: work_to(work, sizes)? },
        host::Answer::Failed { failure, detail, work } => {
            wire::LinkAnswer::Failed { failure: failure_to(failure), detail, work: work_to(work, sizes)? }
        }
    })
}
pub(super) fn finish_from(value: wire::Finish) -> channel::Finish {
    match value {
        wire::Finish::Ended { outcome } => channel::Finish::Ended { outcome },
        wire::Finish::Parked { snapshot } => channel::Finish::Parked { snapshot },
        wire::Finish::Failed { failure } => channel::Finish::Failed { failure: run_from(failure) },
    }
}
pub(super) fn reply_to(value: channel::Reply) -> wire::Reply {
    match value {
        channel::Reply::Relayed { answer } => wire::Reply::Relayed { answer },
        channel::Reply::Pushed(push) => wire::Reply::Pushed {
            push: match push {
                channel::Push::Done => wire::Push::Done,
                channel::Push::Moved => wire::Push::Moved,
                channel::Push::Failed { failure } => wire::Push::Failed { failure: push_failure_to(&failure) },
                channel::Push::Nothing => wire::Push::Nothing,
            },
        },
        channel::Reply::Unavailable => wire::Reply::Unavailable,
        channel::Reply::Busy => wire::Reply::Busy,
        channel::Reply::Withdrawn => wire::Reply::Withdrawn,
        channel::Reply::TooLarge => wire::Reply::TooLarge,
    }
}
pub(super) fn host_push_reason_to(value: host::PushReason) -> wire::PushReason {
    match value {
        host::PushReason::MissingRepository => wire::PushReason::MissingRepository,
        host::PushReason::MissingBranch => wire::PushReason::MissingBranch,
        host::PushReason::MissingCommit => wire::PushReason::MissingCommit,
        host::PushReason::Refused => wire::PushReason::Refused,
        host::PushReason::Unreachable => wire::PushReason::Unreachable,
        host::PushReason::Broken => wire::PushReason::Broken,
        host::PushReason::TimedOut => wire::PushReason::TimedOut,
        host::PushReason::Cancelled => wire::PushReason::Cancelled,
        host::PushReason::Unavailable => wire::PushReason::Unavailable,
        host::PushReason::Busy => wire::PushReason::Busy,
        host::PushReason::TooLarge => wire::PushReason::TooLarge,
        host::PushReason::Nothing => wire::PushReason::Nothing,
        host::PushReason::Unknown => wire::PushReason::Unknown,
    }
}
pub(super) fn host_push_failure_to(value: &host::PushFailure) -> wire::PushFailure {
    wire::PushFailure {
        repository: value.repository,
        reason: host_push_reason_to(value.reason),
        output: copy_of(value.diagnostic.output()),
        cut: value.diagnostic.cut(),
    }
}
