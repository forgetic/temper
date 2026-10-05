//! The channel between the worker and an agent process (worker-domain.md,
//! section 6; agent-domain.md, section 8), as the agent's protocol layer
//! carries it without the bytes of its frames: the worker's channel
//! vocabulary ([`temper_worker_domain_agent::channel`]) on one side, and on the
//! other the agent domain's records toward the worker and its facts. An agent
//! process carries one run, so the channel is the run's: the agent domain
//! knows the worker's side of it by the process's [`Link`], and the run names
//! its host calls by its own tokens for them.
//!
//! ```text
//! worker, down                       agent domain
//! Start, no snapshot                 Start, the charter decoded with the spawn's repositories
//! Start, a snapshot                  - (asserted against: the agent cannot resume a run)
//! Event                              - (dropped: the run hears no inbound events yet)
//! Answer: pushed                     Pushed: done, moved, failed with detail, or nothing to push
//! Answer: unavailable, busy,
//!         too large                  Pushed, failed
//! Answer: withdrawn                  HostCancelled
//! Answer: relayed                    - (asserted against: the run relays no calls)
//! Cancel                             Cancel, once admitted; dropped if the run was refused
//!
//! agent domain                       worker, up
//! Admitted                           - (the run's name, kept for a cancel)
//! Push                               Call, a push with the change's title and body as its message
//! CancelHost                         Withdraw
//! Checking                           Long, the checks' deadline as a span from now
//! fact: checks finished              LongDone
//! fact: anything else                Fact, its kind
//! Answer: accepted                   Finish: ended, the declared outcome encoded
//! Answer: failed                     Finish: failed, as model, budget, policy, cancelled, stale
//! Answer: refused, busy or invalid   Finish: failed, as policy
//! ```
//!
//! Where the two sides do not line up:
//!
//! - A push with nothing to push is failed for the run, which takes it as
//!   feedback for its LLM: a change it declares is the diff of its checkout,
//!   and there is none. So is an answer that says nothing was done: the run
//!   is ending (unavailable), has too many calls in flight (busy, which one
//!   push at a time never meets), or the answer did not fit (too large, which
//!   a push's never is). A withdrawn call is the run's cancel of its push,
//!   having won its race.
//! - A refusal is a run that never began: its charter is beyond the agent's
//!   limits, or the agent has no room for its main conversation, which an
//!   agent that holds only this run has only if its limits hold no run at
//!   all. The channel has no refusal, and a retry of the same charter on an
//!   agent of the same limits is refused again, so it fails as policy: the
//!   run's own failure, which a retry does not get past.
//! - What a run spent has no place on the channel, and is dropped.
//! - Facts go up as their kinds alone, content-free: the worker needs them
//!   only as progress, and the engine counts them. The end of a check is told
//!   as a fact, so its `LongDone` is as lossy as facts are: one dropped
//!   leaves the watchdog paused until the checks' deadline, which bounds it
//!   anyway.
//! - The engine's charter is typed (engine-domain.md, 4.1), and comes as
//!   its codec writes it ([`codec::charter`]). Its brief is sections, which
//!   the LLM reads as text: the plan's guidance first, then why the run is
//!   due, then each section under a heading. Reading is always granted, and
//!   writing and the shell are the run's modify and shell families; the
//!   `note` grant is an outlet of that name. Its budget's tokens are split
//!   across the kinds the agent bounds ([`split`]): half for input, a quarter
//!   for output, an eighth for cache reads and what is left for cache
//!   writes, so that together they spend no more than the engine gave. Its
//!   models are the deployment's names, the first
//!   the main conversation's and the others a sub-agent's to pick, each on
//!   one endpoint and offered [`MAX_TOKENS`]. What it may finish with is a
//!   change, a review's verdicts ("approve", or "request-changes" with one
//!   to eight children), or a report, which the run declares as a verdict
//!   of that name; a session's turn never reaches an agent. Which
//!   repositories are writable is the workspace's, not the charter's.
//! - The outcome goes back as the engine's codec encodes the engine's
//!   outcome ([`codec::outcome`]): a change by its message, a verdict's
//!   children written out after its text.
//!
//! The protocol layer sends a step's facts up before its requests: a check
//! that ends as the next starts says so before the next one's `Long`, and the
//! run's last facts go before its `Finish`, its last word, after which
//! nothing goes up.
//!
//! Not built on the agent's side: inbound events, and the run's waiting for
//! them (`Waiting`); parking and snapshots; relayed calls (forge reads and
//! outlets) and their answers (agent-domain.md, section 10).

use skein_lib::{ReplyTo, Time, Token};
use temper_agent_domain::run::charter::Checkout;
use temper_agent_domain::run::facts as run_facts;
use temper_agent_domain::run::outcome::{Change, Declared};
use temper_agent_domain::run::{self, Budget};
use temper_agent_domain::session;
use temper_agent_domain::tools;
use temper_agent_domain::{Event, Fact, Request};
use temper_legacy_engine_domain::Outcome;
use temper_legacy_engine_domain::plan;
use temper_worker_domain_agent::channel::{Ask, Down, Finish, Push, Reply, RunFailure, Up};

/// What the agent's protocol layer holds for an agent process's channel.
#[derive(Debug)]
pub struct Link {
    /// The agent domain's name for the worker's side of the run, which its
    /// start carries and its answer goes to.
    pub worker: Token,
    /// The repositories of the workspace the process was spawned in, where io
    /// put them: what the protocol layer adds to the charter.
    pub checkout: Checkout,
    /// The run's name, once the agent domain has admitted it.
    pub run: Option<Token>,
}

/// Where a request of the agent domain goes, as its protocol layer routes it.
#[derive(PartialEq, Eq, Debug)]
pub enum Toward {
    /// Up the channel, to the worker.
    Worker(Up),
    /// Nowhere: the run was admitted as `run`, a name the protocol layer keeps
    /// to address a cancel to.
    Admitted { run: Token },
    /// To an LLM provider or to io, as it is.
    Below(Request),
}

/// What the agent domain hears of what the worker sent `link`'s run: nothing,
/// for what it has no ear for yet.
#[must_use]
pub fn down(down: Down, link: &Link) -> Option<Event> {
    match down {
        temper_worker_domain_agent::channel::Down::StartV2 { .. } => unreachable!("this system world runs version one"),

        Down::Start { charter, snapshot, repositories, grants } => {
            assert!(snapshot.is_none(), "the agent never parks, so it is never resumed");
            assert_eq!(repositories.len(), link.checkout.repositories.len());
            for (sent, placed) in repositories.iter().zip(link.checkout.repositories.iter()) {
                assert_eq!(sent.name, placed.name);
                assert_eq!(sent.writable, placed.writable);
            }
            let charter = self::charter(&charter, link.checkout.clone());
            Some(Event::Start {
                grants: grants.into_vec().into_iter().map(grant).collect(),
                reply_to: ReplyTo::new(link.worker),
                worker: link.worker,
                charter,
            })
        }
        Down::Event { name: _, event: _ } => None,
        Down::Grant { grant: named } => Some(Event::Grant { grant: grant(named) }),
        Down::Answer { call, reply } => Some(answer(call, &reply)),
        // A cancel that crossed a refusal finds no run.
        Down::Cancel => link.run.map(|run| Event::Cancel { run }),
    }
}

fn grant(named: temper_worker_domain_agent::channel::Grant) -> temper_agent_domain::Grant {
    temper_agent_domain::Grant {
        name: temper_agent_domain::GrantName { account: named.account, generation: named.generation },
        valid: named.valid,
    }
}

/// The agent domain's terminal for its push `call`, for the worker's answer.
#[must_use]
pub fn answer(call: Token, reply: &Reply) -> Event {
    match reply {
        Reply::Pushed(pushed) => Event::Pushed { owner: call, push: push(pushed) },
        Reply::Unavailable => unpushed(call, run::PushReason::Unavailable),
        Reply::Busy => unpushed(call, run::PushReason::Busy),
        Reply::TooLarge => unpushed(call, run::PushReason::TooLarge),
        Reply::Withdrawn => Event::HostCancelled { owner: call },
        Reply::Relayed { answer: _ } => panic!("the run relays no calls, so no relayed answer comes down"),
    }
}

/// The run's push for the worker's.
#[must_use]
pub fn push(push: &Push) -> run::Push {
    match push {
        temper_worker_domain_agent::channel::Push::Conflicted { .. } => {
            unreachable!("this system world runs version one")
        }

        Push::Done => run::Push::Done,
        Push::Moved => run::Push::Moved,
        Push::Failed { failure } => run::Push::Failed { failure: push_failure(failure) },
        Push::Nothing => run::Push::Nothing,
    }
}

/// Where `request` goes, at `now`.
#[must_use]
pub fn up(request: Request, now: Time) -> Toward {
    match request {
        Request::Admitted { worker: _, run } => Toward::Admitted { run },
        Request::Answer { to: _, answer } => Toward::Worker(Up::Finish { finish: finish(answer) }),
        Request::Checking { worker: _, deadline } => Toward::Worker(Up::Long { span: deadline.saturating_since(now) }),
        Request::Push { worker: _, owner, change } => {
            Toward::Worker(Up::Call { call: owner, ask: Ask::Push { message: message(&change) } })
        }
        Request::CancelHost { owner } => Toward::Worker(Up::Withdraw { call: owner }),
        Request::Rejected { grant } => {
            Toward::Worker(Up::Rejected { account: grant.account, generation: grant.generation })
        }
        Request::Exhausted { account, retry_after } => Toward::Worker(Up::Exhausted { account, retry_after }),
        below @ (Request::Complete { .. }
        | Request::Cancel { .. }
        | Request::Io { .. }
        | Request::CancelIo { .. }
        | Request::Read { .. }
        | Request::Probe { .. }
        | Request::Check { .. }
        | Request::Abort { .. }) => Toward::Below(below),
    }
}

/// How the run finishes, for its answer.
#[must_use]
pub fn finish(answer: run::Answer) -> Finish {
    match answer {
        run::Answer::Accepted { outcome: declared, spent: _ } => Finish::Ended { outcome: self::outcome(&declared) },
        run::Answer::Failed { failure, spent: _ } => Finish::Failed { failure: self::failure(failure) },
        run::Answer::Refused(run::Refusal::Busy | run::Refusal::Invalid(_)) => {
            Finish::Failed { failure: RunFailure::Policy }
        }
    }
}

fn failure(failure: run::Failure) -> RunFailure {
    match failure {
        run::Failure::Model(run::Fault::Exhausted) => RunFailure::Exhausted,
        run::Failure::Model(
            run::Fault::Provider
            | run::Fault::ContextFull
            | run::Fault::Refused
            | run::Fault::Truncated
            | run::Fault::Malformed,
        ) => RunFailure::Model,
        run::Failure::Budget(_) => RunFailure::Budget,
        run::Failure::Policy(_) => RunFailure::Policy,
        run::Failure::Cancelled => RunFailure::Cancelled,
        run::Failure::Stale => RunFailure::Stale,
    }
}

/// A push's commit message: the change's title, then its body after a blank
/// line, if it has one.
fn message(change: &Change) -> Box<[u8]> {
    temper_agent_protocol::payload::message(change, temper_channel::Sizes::STARTING.detail)
        .expect("the domain-tier push message fits")
}

/// What goes up for `fact`.
#[must_use]
pub fn fact(fact: Fact) -> Up {
    let kind = match fact {
        Fact::Run { fact } => run_fact(&fact),
        Fact::Session { fact } => Some(session_fact(fact)),
    };
    match kind {
        Some(kind) => Up::Fact { fact: kind.into() },
        None => Up::LongDone,
    }
}

/// The kind of a run's fact; none for the end of a check, which is told as
/// such.
fn run_fact(fact: &run_facts::Fact) -> Option<&'static [u8]> {
    let kind: &[u8] = match fact {
        run_facts::Fact::Admitted { .. } => b"run.admitted",
        run_facts::Fact::Prepared { .. } => b"run.prepared",
        run_facts::Fact::Opened { .. } => b"run.opened",
        run_facts::Fact::Ended { .. } => b"run.ended",
        run_facts::Fact::Called { .. } => b"run.called",
        run_facts::Fact::Returned { .. } => b"run.returned",
        run_facts::Fact::CheckStarted { .. } => b"run.check-started",
        run_facts::Fact::CheckFinished { .. } => return None,
        run_facts::Fact::Pushed { .. } => b"run.pushed",
        run_facts::Fact::Answered { .. } => b"run.answered",
    };
    Some(kind)
}

fn session_fact(fact: session::Fact) -> &'static [u8] {
    match fact {
        session::Fact::Opened { .. } => b"session.opened",
        session::Fact::CompletionStarted { .. } => b"session.completion-started",
        session::Fact::CompletionAnswered { .. } => b"session.completion-answered",
        session::Fact::CompletionFailed { .. } => b"session.completion-failed",
        session::Fact::CompletionCancelled { .. } => b"session.completion-cancelled",
        session::Fact::CompletionRetried { .. } => b"session.completion-retried",
        session::Fact::Tools { opener: _, fact } => tools_fact(fact),
        session::Fact::DelegateStarted { .. } => b"session.delegate-started",
        session::Fact::DelegateAnswered { .. } => b"session.delegate-answered",
        session::Fact::DelegateCancelled { .. } => b"session.delegate-cancelled",
        session::Fact::Yielded { .. } => b"session.yielded",
        session::Fact::Used { .. } => b"session.used",
        session::Fact::Ended { .. } => b"session.ended",
    }
}

fn tools_fact(fact: tools::Fact) -> &'static [u8] {
    match fact {
        tools::Fact::Opened { .. } => b"tools.opened",
        tools::Fact::Refused { .. } => b"tools.refused",
        tools::Fact::Started { .. } => b"tools.started",
        tools::Fact::Answered { .. } => b"tools.answered",
        tools::Fact::Closing { .. } => b"tools.closing",
        tools::Fact::Closed { .. } => b"tools.closed",
    }
}

/// The model every LLM of a run is offered at most this many tokens of, as
/// the deployment configures the agent's protocol layer.
pub const MAX_TOKENS: u32 = 1024;

/// The run's charter for the engine's, as the engine's codec encodes it
/// ([`codec::charter`]), with `checkout`'s repositories. Panics on bytes that
/// do not decode: the engine's side encoded them.
#[must_use]
pub fn charter(bytes: &[u8], checkout: Checkout) -> run::Charter {
    // Domain-tier deployment wiring supplies account zero's dummy descriptors;
    // production resolves real indexed descriptors at its channel entrance.
    let wire = temper_channel::payload::v1::decode_charter(bytes, &temper_channel::Sizes::STARTING)
        .expect("a charter decodes as the engine's side encoded it");
    let mut endpoints = Vec::new();
    for model in wire.models {
        if !endpoints.iter().any(|other: &temper_channel::wire::EndpointDescriptor| other.endpoint == model.endpoint) {
            endpoints.push(temper_channel::wire::EndpointDescriptor {
                endpoint: model.endpoint,
                provider: temper_channel::wire::Provider::OpenAi,
                host: b"localhost".as_slice().into(),
                address: temper_channel::wire::Address::V4 { bytes: [127, 0, 0, 1] },
                port: 8000,
                path: b"/responses".as_slice().into(),
                account: 0,
                effort: Box::new([]),
                thinking: None,
            });
        }
    }
    temper_agent_protocol::payload::charter(
        bytes,
        checkout,
        &endpoints,
        &temper_channel::Sizes::STARTING,
        &crate::translate::limits(),
    )
    .expect("the domain-tier charter is representable by the current agent")
}

/// The outlet of the engine's note grant.
pub const NOTE: &[u8] = b"note";
#[must_use]
pub fn split(budget: plan::Budget) -> Budget {
    Budget::from_tokens(budget.turns, budget.tokens, budget.time)
}
pub const REPORT: &[u8] = temper_agent_protocol::payload::REPORT;
pub const APPROVE: &[u8] = temper_agent_protocol::payload::APPROVE;
pub const REQUEST: &[u8] = temper_agent_protocol::payload::REQUEST;

#[must_use]
pub fn outcome(declared: &Declared) -> Box<[u8]> {
    temper_agent_protocol::payload::outcome(declared, &temper_channel::Sizes::STARTING)
        .expect("the declared outcome fits the domain-tier deployment")
}
#[must_use]
pub fn engine_outcome(declared: &Declared) -> Outcome {
    temper_engine_protocol::payload::decode_outcome(&outcome(declared), &temper_channel::Sizes::STARTING)
        .expect("production outcome encoding decodes")
}

/// The run retains the worker's reason, repository and diagnostic tail.
fn push_failure(failure: &temper_worker_domain_agent::PushFailure) -> run::PushFailure {
    use temper_worker_domain_agent::PushReason;
    let reason = match failure.reason {
        PushReason::MissingRepository => run::PushReason::MissingRepository,
        PushReason::MissingBranch => run::PushReason::MissingBranch,
        PushReason::MissingCommit => run::PushReason::MissingCommit,
        PushReason::Refused => run::PushReason::Refused,
        PushReason::Unreachable => run::PushReason::Unreachable,
        PushReason::Broken => run::PushReason::Broken,
        PushReason::TimedOut => run::PushReason::TimedOut,
        PushReason::Cancelled => run::PushReason::Cancelled,
        PushReason::Unavailable => run::PushReason::Unavailable,
        PushReason::Busy => run::PushReason::Busy,
        PushReason::TooLarge => run::PushReason::TooLarge,
        PushReason::Nothing => run::PushReason::Nothing,
        PushReason::Unknown => run::PushReason::Unknown,
    };
    run::PushFailure {
        repository: failure.repository,
        reason,
        diagnostic: run::PushDiagnostic::new(failure.diagnostic.output(), failure.diagnostic.cut()),
    }
}

fn unpushed(owner: Token, reason: run::PushReason) -> Event {
    Event::Pushed { owner, push: run::Push::Failed { failure: run::PushFailure::new(reason) } }
}
