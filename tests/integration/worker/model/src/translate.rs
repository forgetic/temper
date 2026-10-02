//! Between the worker and its neighbours, as the protocol layers would
//! translate: the fake engine's api ([`temper_fake_engine_model::api`]) and
//! the worker's boundary, each in its own terms; and io's agent processes,
//! as the agent world's process trees speak the agent sub-model's io.
//!
//! Names pass as they are: the engine's tokens for a run and an attempt are
//! the worker's, and its bytes the worker's. A read-only repository reaches
//! the forge as the engine's one identity, [`IDENTITY`], like a writable one.
//! A commit's hash becomes the fixed-size value the worker compares: the
//! world's forge names its commits by a count, and the world gives the hash
//! the engine names, [`COMMIT`], the count of a commit of its choosing.
//!
//! What the worker passes through unread, the world frames on its way in, as
//! a protocol layer may: a charter begins with the name of the attempt it is
//! for, and an inbound event with its place among the events the engine sent
//! that attempt, then the attempt's name. So the world can tell, at the agent,
//! which attempt a message was meant for, and the scripted agent that its
//! events come once each, in order.

use std::collections::BTreeMap;

use temper_fake_engine_model::{self as engine, COMMIT, IDENTITY, api};
use temper_lib::Token;
use temper_worker_model::agent::{self, channel::Down};
use temper_worker_model::checkout::git::Commit;
use temper_worker_model::{Event, Hello, Hosted, Phase, Told, host};

/// The bytes a frame adds to a charter.
pub const CHARTER_FRAME: u64 = 8;

/// The bytes a frame adds to an inbound event.
pub const EVENT_FRAME: u64 = 16;

/// The worker's assignment for the engine's, its charter framed, the hash
/// [`COMMIT`] standing for `commit`.
#[must_use]
pub fn assignment(assignment: api::Assignment, commit: Commit) -> host::Assignment {
    let api::Assignment { run, attempt, workspace, save, charter, snapshot } = assignment;
    let api::Workspace { key, repositories } = workspace;
    let repositories = repositories.into_vec().into_iter().map(|repository| self::repository(repository, commit));
    host::Assignment {
        run,
        attempt,
        workspace: host::Workspace { key, repositories: repositories.collect() },
        save,
        charter: framed_charter(attempt, &charter),
        snapshot,
    }
}

fn repository(repository: api::Repository, commit: Commit) -> host::Repository {
    let api::Repository { name, remote, start, access } = repository;
    let start = match start {
        api::Start::Base { branch } => host::Start::Base { branch },
        api::Start::Branch { branch } => host::Start::Branch { branch },
        api::Start::Commit { commit: hash } => {
            assert_eq!(&*hash, COMMIT, "the engine names the one commit it knows");
            host::Start::Commit { commit: commit.raw() }
        }
        api::Start::Saved { branch } => host::Start::Saved { branch },
    };
    let (access, identity) = match access {
        api::Access::ReadOnly => (host::Access::ReadOnly, IDENTITY.into()),
        api::Access::Writable { push, identity } => (host::Access::Writable { push }, identity),
    };
    host::Repository { name, remote, start, access, identity }
}

/// `charter`, framed with the name of the attempt it is for.
#[must_use]
pub fn framed_charter(attempt: Token, charter: &[u8]) -> Box<[u8]> {
    [attempt.raw().to_be_bytes().as_slice(), charter].concat().into_boxed_slice()
}

/// The attempt a framed charter is for.
#[must_use]
pub fn charter_attempt(charter: &[u8]) -> Token {
    let name = charter.get(..8).expect("a charter begins with its frame");
    Token::new(u64::from_be_bytes(name.try_into().expect("eight bytes")))
}

/// `event`, framed with its place among the events sent to `attempt`, and
/// the attempt's name.
#[must_use]
pub fn framed_event(place: u64, attempt: Token, event: &[u8]) -> Box<[u8]> {
    [place.to_be_bytes().as_slice(), &attempt.raw().to_be_bytes(), event].concat().into_boxed_slice()
}

/// The attempt a framed inbound event was sent to.
#[must_use]
pub fn event_attempt(event: &[u8]) -> Token {
    let name = event.get(8..16).expect("an event begins with its frame");
    Token::new(u64::from_be_bytes(name.try_into().expect("eight bytes")))
}

/// What the worker hears of the engine's `request`, sent to it over the
/// channel: charters and events framed, an event with its place among those
/// sent to its attempt, counted in `places`; the hash [`COMMIT`] standing
/// for `commit`.
#[must_use]
pub fn down(request: engine::Request, commit: Commit, places: &mut BTreeMap<Token, u64>) -> Event {
    match request {
        engine::Request::Assign { worker: _, assignment } => {
            Event::Assign { assignment: self::assignment(assignment, commit) }
        }
        engine::Request::Inbound { worker: _, run, attempt, event } => {
            let place = places.entry(attempt).or_default();
            let event = framed_event(*place, attempt, &event);
            *place += 1;
            Event::Inbound { run, attempt, event }
        }
        engine::Request::Cancel { worker: _, run, attempt } => Event::Cancel { run, attempt },
        engine::Request::Relayed { worker: _, run, attempt, call, answer } => {
            Event::Relayed { run, attempt, call, answer }
        }
        engine::Request::Acknowledge { worker: _, run, attempt } => Event::Acknowledged { run, attempt },
    }
}

/// What the engine hears of a fact the worker `worker` forwarded: that there
/// was one.
#[must_use]
pub fn told(told: &Told, worker: Token) -> engine::Event {
    engine::Event::Fact { worker, run: told.run, attempt: told.attempt }
}

/// The engine's hello for the worker's.
#[must_use]
pub fn hello(hello: Hello) -> api::Hello {
    let Hello { slots, workstreams, hosting } = hello;
    let hosting =
        hosting.iter().map(|&Hosted { run, attempt, phase }| api::Hosted { run, attempt, phase: self::phase(phase) });
    api::Hello { slots, workstreams, hosting: hosting.collect() }
}

fn phase(phase: Phase) -> api::Phase {
    match phase {
        Phase::Preparing => api::Phase::Preparing,
        Phase::Starting => api::Phase::Starting,
        Phase::Active => api::Phase::Active,
        Phase::Waiting => api::Phase::Waiting,
        Phase::Ending => api::Phase::Ending,
        Phase::Answered => api::Phase::Answered,
    }
}

/// The engine's answer for the worker's. The detail of a failure is for
/// operators: the engine's api has no room for it.
#[must_use]
pub fn answer(answer: host::Answer) -> api::Answer {
    match answer {
        host::Answer::Refused(refusal) => api::Answer::Refused(self::refusal(refusal)),
        host::Answer::Ended { outcome, work } => api::Answer::Ended { outcome, work: self::work(work) },
        host::Answer::Parked { snapshot, work } => api::Answer::Parked { snapshot, work: self::work(work) },
        host::Answer::Failed { failure, detail: _, work } => {
            api::Answer::Failed { failure: self::failure(failure), work: self::work(work) }
        }
    }
}

fn refusal(refusal: host::Refusal) -> api::Refusal {
    match refusal {
        host::Refusal::Busy => api::Refusal::Busy,
        host::Refusal::Invalid(invalid) => api::Refusal::Invalid(match invalid {
            host::Invalid::Repositories => api::Invalid::Repositories,
            host::Invalid::Duplicate => api::Invalid::Duplicate,
            host::Invalid::Name => api::Invalid::Name,
            host::Invalid::Charter => api::Invalid::Charter,
            host::Invalid::Snapshot => api::Invalid::Snapshot,
        }),
    }
}

fn work(work: host::Work) -> api::Work {
    let host::Work { landed, saved } = work;
    let landed = landed.iter().map(|landed| landed.repository).collect();
    let saved = saved.map(|saved| saved.iter().map(|&landing| self::landing(landing)).collect());
    api::Work { landed, saved }
}

/// The engine's landing for the worker's: it has no word for a commit landed,
/// nor for a push the forge refused, which failed as far as it knows.
fn landing(landing: host::Landing) -> api::Landing {
    match landing {
        host::Landing::Landed { commit: _ } => api::Landing::Landed,
        host::Landing::Moved => api::Landing::Moved,
        host::Landing::Failed | host::Landing::Refused => api::Landing::Failed,
        host::Landing::Unchanged => api::Landing::Unchanged,
    }
}

/// The engine's failure for the worker's: what it has no word for, a
/// preparation that failed for a repository the forge does not have or that
/// refused its identity, is permanent as far as it knows.
fn failure(failure: host::Failure) -> api::Failure {
    match failure {
        host::Failure::Unprepared(preparation) => api::Failure::Unprepared(match preparation {
            host::Preparation::Transient => api::Preparation::Transient,
            host::Preparation::Missing { .. } | host::Preparation::Refused { .. } => api::Preparation::Permanent,
        }),
        host::Failure::Run(run) => api::Failure::Run(match run {
            host::RunFailure::Model => api::RunFailure::Model,
            host::RunFailure::Budget => api::RunFailure::Budget,
            host::RunFailure::Policy => api::RunFailure::Policy,
            host::RunFailure::Cancelled => api::RunFailure::Cancelled,
            host::RunFailure::Stale => api::RunFailure::Stale,
        }),
        host::Failure::Agent(agent) => api::Failure::Agent(match agent {
            host::AgentFailure::Unstarted => api::AgentFailure::Unstarted,
            host::AgentFailure::Exited => api::AgentFailure::Exited,
            host::AgentFailure::Rules => api::AgentFailure::Rules,
            host::AgentFailure::NoProgress => api::AgentFailure::NoProgress,
            host::AgentFailure::WallTime => api::AgentFailure::WallTime,
        }),
        host::Failure::Cancelled(reason) => api::Failure::Cancelled(match reason {
            host::Reason::Engine => api::Cause::Engine,
            host::Reason::Contact => api::Cause::Contact,
            host::Reason::Shutdown => api::Cause::Shutdown,
        }),
    }
}

/// The engine's name for the worker's bounce.
#[must_use]
pub fn bounce(bounce: host::Bounce) -> api::Bounce {
    match bounce {
        host::Bounce::TooLarge => api::Bounce::TooLarge,
        host::Bounce::Full => api::Bounce::Full,
        host::Bounce::Ending => api::Bounce::Ending,
    }
}

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

/// The worker's event for the agent sub-model's io terminal `event`.
#[must_use]
pub fn from_agent_io(event: agent::Event) -> Event {
    match event {
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
        | agent::Event::Stop { .. } => {
            unreachable!("io ends requests")
        }
    }
}

/// The bytes `message` carries down to an agent, for the world to look into.
#[must_use]
pub fn down_bytes(message: &Down) -> Vec<&[u8]> {
    match message {
        Down::Start { charter, snapshot } => [Some(&**charter), snapshot.as_deref()].into_iter().flatten().collect(),
        Down::Event { event } => vec![event],
        Down::Answer { reply, .. } => match reply {
            agent::channel::Reply::Relayed { answer } => vec![answer],
            agent::channel::Reply::Pushed(_)
            | agent::channel::Reply::Unavailable
            | agent::channel::Reply::Busy
            | agent::channel::Reply::Withdrawn
            | agent::channel::Reply::TooLarge => Vec::new(),
        },
        Down::Cancel => Vec::new(),
    }
}
