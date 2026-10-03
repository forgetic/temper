//! The channel between the engine and the worker (worker-domain.md, sections
//! 2 and 4; engine-domain.md, section 8), as the protocol layers on both
//! sides would carry it: the engine's boundary ([`temper_engine_domain`]) on
//! one side, the worker's ([`temper_worker_domain`]) on the other, each in its
//! own terms.
//!
//! ```text
//! engine                             worker
//! Assign                             Assign: the workspace's repositories by name, remote and
//!                                    identity, the charter encoded and framed
//! Inbound                            Inbound: the event, named by its kind
//! Cancel, Acknowledge                Cancel, Acknowledged
//! Relayed                            - (asserted against: the agent's runs relay no calls)
//!
//! worker                             engine
//! Hello                              Hello: each run hosted by its item and attempt
//! Answer                             Answer: the outcome decoded, what landed by the deployment's
//!                                    index, the failure by its class
//! Bounced                            Bounced
//! a fact the run told                Told, its kind by what the fact is about
//! Relay                              - (asserted against: the agent's runs relay no calls)
//! ```
//!
//! Names are packed into the channel's tokens as the engine's world packs
//! them for every system world ([`temper_engine_domain_tests::names`]), so
//! that every attempt of every item has a name of its own. A repository of a
//! workspace is the deployment's, by its forge name: the worker puts it in a
//! directory named for its last component, and reaches it as the one
//! identity the deployment gives its workers, [`IDENTITY`].
//!
//! The charter goes as the engine's codec writes it ([`codec::charter`]),
//! framed on its way in with the name of the attempt it is for, which the
//! agent's protocol layer takes off ([`CHARTER_FRAME`]), so that the world can
//! tell at the agent which attempt a start was meant for. The outcome comes
//! back as the agent's protocol layer encodes it, with the same codec
//! ([`crate::channel::outcome`]).
//!
//! Where the two sides do not line up:
//!
//! - The engine classes failures (engine-domain.md, 4.3): a workspace that
//!   could not be prepared for a while is transient, and so is a run the
//!   worker cancelled, for the engine or because it lost contact or shuts
//!   down; one the forge does not have what it names for, or refused, is
//!   permanent; the run's own failures are the run's, and the agent's the
//!   agent's. The detail of a failure is for operators, and dropped.
//! - What a run saved has no place in the engine's answer: the engine finds
//!   its saved-work branch on the forge.
//! - A fact goes up as its kind: progress, a call, a tool or a check, usage.

use temper_engine_domain::views::Kind;
use temper_engine_domain::{self as engine, Assignment};
use temper_engine_domain_tests::{codec, deployment};
use temper_lib::Token;
use temper_worker_domain::{self as worker, Told, host};

/// Who the worker is to the forge, for every repository it checks out.
pub const IDENTITY: &[u8] = b"worker";

/// The bytes a frame adds to a charter.
pub const CHARTER_FRAME: usize = 8;

pub use temper_engine_domain_tests::names::{attempt, attempt_of, run};

/// The directory a repository of the deployment sits in: the last component
/// of its forge name.
#[must_use]
pub fn directory(repository: u32) -> &'static [u8] {
    let name = deployment::name(repository);
    let at = name.iter().rposition(|byte| *byte == b'/').map_or(0, |slash| slash + 1);
    &name[at..]
}

/// The worker's assignment for the engine's, its charter encoded and framed;
/// and the deployment's index for each repository of its workspace, in the
/// workspace's order, which an answer's landings name by place.
#[must_use]
pub fn assignment(assignment: Assignment) -> (host::Assignment, Vec<u32>) {
    let Assignment { item, attempt, workspace, save, charter, snapshot } = assignment;
    let name = self::attempt(item, attempt);
    let places = workspace.repositories.iter().map(|checkout| checkout.repository).collect();
    let repositories = workspace.repositories.into_vec().into_iter().map(repository).collect();
    let assignment = host::Assignment {
        run: run(item),
        attempt: name,
        workspace: host::Workspace { key: workspace.key, repositories },
        save,
        charter: framed_charter(name, &codec::charter(&charter)),
        snapshot,
    };
    (assignment, places)
}

fn repository(checkout: engine::Checkout) -> host::Repository {
    let engine::Checkout { repository, start, push } = checkout;
    let start = match start {
        engine::Start::Base { branch } => host::Start::Base { branch },
        engine::Start::Branch { branch } => host::Start::Branch { branch },
        engine::Start::Commit { commit } => host::Start::Commit { commit },
        engine::Start::Saved { branch } => host::Start::Saved { branch },
    };
    let access = match push {
        Some(push) => host::Access::Writable { push },
        None => host::Access::ReadOnly,
    };
    host::Repository {
        name: directory(repository).into(),
        remote: deployment::name(repository).into(),
        start,
        access,
        identity: IDENTITY.into(),
    }
}

/// `charter`, framed with the name of the attempt it is for.
#[must_use]
pub fn framed_charter(attempt: Token, charter: &[u8]) -> Box<[u8]> {
    [attempt.raw().to_be_bytes().as_slice(), charter].concat().into_boxed_slice()
}

/// The attempt a framed charter is for, and the charter.
#[must_use]
pub fn unframed(charter: &[u8]) -> (Token, &[u8]) {
    let (frame, charter) = charter.split_at_checked(CHARTER_FRAME).expect("a charter begins with its frame");
    (Token::new(u64::from_be_bytes(frame.try_into().expect("eight bytes"))), charter)
}

/// What the worker hears of the engine's `request` for a worker: none for
/// what goes to no worker.
#[must_use]
pub fn down(request: &engine::Request) -> Option<worker::Event> {
    let event = match *request {
        engine::Request::Assign { .. } => unreachable!("the world translates an assignment with its places"),
        engine::Request::Inbound { channel: _, item, attempt, event } => {
            worker::Event::Inbound { run: run(item), attempt: self::attempt(item, attempt), event: inbound(event) }
        }
        engine::Request::Cancel { channel: _, item, attempt } => {
            worker::Event::Cancel { run: run(item), attempt: self::attempt(item, attempt) }
        }
        engine::Request::Relayed { .. } => unreachable!("the agent's runs relay no calls, so none is answered"),
        engine::Request::Acknowledge { channel: _, item, attempt } => {
            worker::Event::Acknowledged { run: run(item), attempt: self::attempt(item, attempt) }
        }
        engine::Request::Forge { .. }
        | engine::Request::Refuse { .. }
        | engine::Request::Reply { .. }
        | engine::Request::Deliver { .. }
        | engine::Request::Ended { .. }
        | engine::Request::Store { .. } => return None,
    };
    Some(event)
}

/// An inbound event's bytes: its kind. The agent hears none yet.
fn inbound(event: engine::Inbound) -> Box<[u8]> {
    let kind: &[u8] = match event {
        engine::Inbound::News(_) => b"news",
        engine::Inbound::Finished { .. } => b"finished",
        engine::Inbound::Held { .. } => b"held",
        engine::Inbound::Decided { .. } => b"decided",
    };
    kind.into()
}

/// The engine's hello for the worker's.
#[must_use]
pub fn hello(hello: worker::Hello) -> engine::Hello {
    let worker::Hello { slots, workstreams, hosting } = hello;
    let hosting = hosting.iter().map(|hosted| {
        let (item, attempt) = attempt_of(hosted.attempt);
        engine::Hosted { item, attempt, phase: phase(hosted.phase) }
    });
    engine::Hello { slots, workstreams, hosting: hosting.collect() }
}

fn phase(phase: worker::Phase) -> engine::fleet::Phase {
    match phase {
        worker::Phase::Preparing => engine::fleet::Phase::Preparing,
        worker::Phase::Starting => engine::fleet::Phase::Starting,
        worker::Phase::Active => engine::fleet::Phase::Active,
        worker::Phase::Waiting => engine::fleet::Phase::Waiting,
        worker::Phase::Ending => engine::fleet::Phase::Ending,
        worker::Phase::Answered => engine::fleet::Phase::Answered,
    }
}

/// The engine's answer for the worker's, its landings named by the
/// deployment's index for each repository of the workspace, `places`.
#[must_use]
pub fn answer(answer: host::Answer, places: &[u32]) -> engine::Answer {
    let work = |work: host::Work| {
        let landed = work.landed.iter().map(|landed| engine::Landed {
            repository: places[usize::try_from(landed.repository).expect("a small place")],
            commit: landed.commit,
        });
        engine::Work { landed: landed.collect() }
    };
    match answer {
        host::Answer::Refused(host::Refusal::Busy) => engine::Answer::Busy,
        host::Answer::Refused(host::Refusal::Invalid(_)) => engine::Answer::Invalid,
        host::Answer::Ended { outcome, work: done } => {
            let outcome = codec::outcome_of(&outcome).expect("an outcome decodes as the agent's side encoded it");
            engine::Answer::Ended { outcome, work: work(done) }
        }
        host::Answer::Parked { snapshot, work: done } => engine::Answer::Parked { snapshot, work: work(done) },
        host::Answer::Failed { failure, detail: _, work: done } => {
            engine::Answer::Failed { failure: self::failure(failure), work: work(done) }
        }
    }
}

/// The engine's class for the worker's failure.
#[must_use]
pub fn failure(failure: host::Failure) -> engine::Failure {
    match failure {
        host::Failure::Unprepared(host::Preparation::Transient) | host::Failure::Cancelled(_) => {
            engine::Failure::Transient
        }
        host::Failure::Unprepared(host::Preparation::Missing { .. } | host::Preparation::Refused { .. }) => {
            engine::Failure::Permanent
        }
        host::Failure::Run(_) => engine::Failure::Run,
        host::Failure::Agent(_) => engine::Failure::Agent,
    }
}

/// The engine's name for the worker's bounce.
#[must_use]
pub fn bounce(bounce: host::Bounce) -> engine::fleet::Bounce {
    match bounce {
        host::Bounce::TooLarge => engine::fleet::Bounce::TooLarge,
        host::Bounce::Full => engine::fleet::Bounce::Full,
        host::Bounce::Ending => engine::fleet::Bounce::Ending,
    }
}

/// What the engine hears of a fact a run told: its kind, by what it is
/// about, and the fact as it came.
#[must_use]
pub fn told(told: Told) -> engine::Event {
    let Told { run: _, attempt, fact } = told;
    let (item, attempt) = attempt_of(attempt);
    let kind = if fact.starts_with(b"session.completion") {
        Kind::Call
    } else if fact.starts_with(b"tools.") || fact.starts_with(b"run.check") {
        Kind::Tool
    } else if &*fact == b"session.used" {
        Kind::Usage
    } else {
        Kind::Progress
    };
    engine::Event::Told { item, attempt, kind, content: fact }
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
