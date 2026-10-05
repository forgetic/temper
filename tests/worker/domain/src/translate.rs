//! Between the worker and the agent processes it hosts, as io's protocol
//! layer would translate: the agent world's process trees speak the agent
//! child domain's io, and the worker's boundary carries it. And the names the
//! world counts the worker's answers by.

use temper_worker_domain::agent::{self, channel::Down};
use temper_worker_domain::{Event, host};

/// What kind of answer `answer` is, for the world's counts: a refusal or a
/// failure by its reason, else how the run ended.
#[must_use]
pub fn answer_kind(answer: &host::Answer) -> &'static str {
    match answer {
        host::Answer::Refused(host::Refusal::Busy) => "refused busy",
        host::Answer::Refused(host::Refusal::Invalid(_)) => "refused invalid",
        host::Answer::Ended { .. } => "ended",
        host::Answer::Parked { .. } => "parked",
        host::Answer::Failed { failure, .. } => failure_kind(*failure),
    }
}

/// What kind of failure `failure` is, for the world's counts.
#[must_use]
pub fn failure_kind(failure: host::Failure) -> &'static str {
    match failure {
        host::Failure::Unprepared(host::Preparation::Transient) => "unprepared transient",
        host::Failure::Unprepared(host::Preparation::Missing { .. }) => "unprepared missing",
        host::Failure::Unprepared(host::Preparation::Refused { .. }) => "unprepared refused",
        host::Failure::Run(host::RunFailure::Model) => "run model",
        host::Failure::Run(host::RunFailure::Budget) => "run budget",
        host::Failure::Run(host::RunFailure::Policy) => "run policy",
        host::Failure::Run(host::RunFailure::Cancelled) => "run cancelled",
        host::Failure::Run(host::RunFailure::Stale) => "run stale",
        host::Failure::Run(host::RunFailure::Exhausted) => "run exhausted",
        host::Failure::Agent(host::AgentFailure::Unstarted) => "agent unstarted",
        host::Failure::Agent(host::AgentFailure::Exited) => "agent exited",
        host::Failure::Agent(host::AgentFailure::Rules) => "agent rules",
        host::Failure::Agent(host::AgentFailure::NoProgress) => "agent no progress",
        host::Failure::Agent(host::AgentFailure::WallTime) => "agent wall time",
        host::Failure::Cancelled(host::Reason::Engine) => "cancelled engine",
        host::Failure::Cancelled(host::Reason::Contact) => "cancelled contact",
        host::Failure::Cancelled(host::Reason::Shutdown) => "cancelled shutdown",
    }
}

/// Every kind [`answer_kind`] names, for a sweep to check it reached them.
pub const ANSWER_KINDS: [&str; 20] = [
    "refused busy",
    "refused invalid",
    "ended",
    "parked",
    "unprepared transient",
    "unprepared missing",
    "unprepared refused",
    "run model",
    "run budget",
    "run policy",
    "run cancelled",
    "run stale",
    "agent unstarted",
    "agent exited",
    "agent rules",
    "agent no progress",
    "agent wall time",
    "cancelled engine",
    "cancelled contact",
    "cancelled shutdown",
];

/// The worker's event for the agent child domain's io terminal `event`.
#[must_use]
pub fn from_agent_io(event: agent::Event) -> Event {
    match event {
        temper_worker_domain::agent::Event::SpawnV2 { .. } | temper_worker_domain::agent::Event::TurnCredit { .. } => {
            unreachable!("this system world runs version one")
        }

        agent::Event::Spawned { owner, process } => Event::Spawned { owner, process },
        agent::Event::Unspawned { owner, detail } => Event::Unspawned { owner, detail },
        agent::Event::Sent { owner } => Event::Sent { owner },
        agent::Event::Unsent { owner } => Event::Unsent { owner },
        agent::Event::Received { owner, message } => Event::Received { owner, message },
        agent::Event::Malformed { owner } => Event::Malformed { owner },
        agent::Event::Hangup { owner } => Event::Hangup { owner },
        agent::Event::Signalled { owner } => Event::Signalled { owner },
        agent::Event::Exited { owner } => Event::Exited { owner },
        agent::Event::Reaped { owner, detail } => Event::Reaped { owner, detail },
        agent::Event::Spawn { .. }
        | agent::Event::Deliver { .. }
        | agent::Event::Answer { .. }
        | agent::Event::Stop { .. }
        | agent::Event::Grant { .. } => {
            unreachable!("io ends requests")
        }
    }
}

/// The bytes `message` carries down to an agent, for the world to look into.
#[must_use]
pub fn down_bytes(message: &Down) -> Vec<&[u8]> {
    match message {
        temper_worker_domain::agent::channel::Down::StartV2 { .. } => {
            unreachable!("this system world runs version one")
        }

        Down::Start { repositories: _, grants: _, charter, snapshot } => {
            [Some(&**charter), snapshot.as_deref()].into_iter().flatten().collect()
        }
        Down::Event { name: _, event } => vec![event],
        Down::Answer { reply, .. } => match reply {
            agent::channel::Reply::Relayed { answer } => vec![answer],
            agent::channel::Reply::Pushed(_)
            | agent::channel::Reply::Unavailable
            | agent::channel::Reply::Busy
            | agent::channel::Reply::Withdrawn
            | agent::channel::Reply::TooLarge => Vec::new(),
        },
        Down::Cancel | Down::Grant { .. } => Vec::new(),
    }
}
