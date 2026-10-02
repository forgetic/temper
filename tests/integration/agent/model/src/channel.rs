//! The channel between the worker and an agent process (worker-model.md,
//! section 6; agent-model.md, section 8), as the agent's protocol layer
//! carries it without the bytes of its frames: the worker's channel
//! vocabulary ([`temper_worker_model_agent::channel`]) on one side, and on the
//! other the agent model's records toward the worker and its facts. An agent
//! process carries one run, so the channel is the run's: the agent model
//! knows the worker's side of it by the process's [`Link`], and the run names
//! its host calls by its own tokens for them.
//!
//! ```text
//! worker, down                       agent model
//! Start, no snapshot                 Start, the charter decoded with the spawn's repositories
//! Start, a snapshot                  - (asserted against: the agent cannot resume a run)
//! Event                              - (dropped: the run hears no inbound events yet)
//! Answer: pushed                     Pushed: done, moved, failed; nothing to push as failed
//! Answer: unavailable, busy,
//!         too large                  Pushed, failed
//! Answer: withdrawn                  HostCancelled
//! Answer: relayed                    - (asserted against: the run relays no calls)
//! Cancel                             Cancel, once admitted; dropped if the run was refused
//!
//! agent model                        worker, up
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
//! - The engine's charter names one endpoint and one `max_tokens` for every
//!   model, which each sub-agent's LLM takes from the main one; it grants
//!   reading and writing, which are the run's inspect and modify families;
//!   and whether a change must pass its checks means nothing without a
//!   change. Which repositories are writable is the workspace's, not the
//!   charter's.
//!
//! The protocol layer sends a step's facts up before its requests: a check
//! that ends as the next starts says so before the next one's `Long`, and the
//! run's last facts go before its `Finish`, its last word, after which
//! nothing goes up.
//!
//! Not built on the agent's side: inbound events, and the run's waiting for
//! them (`Waiting`); parking and snapshots; relayed calls (forge reads and
//! outlets) and their answers (agent-model.md, section 10).

use temper_agent_model::run::charter::{self, Checkout};
use temper_agent_model::run::facts as run_facts;
use temper_agent_model::run::outcome::VerdictRule;
use temper_agent_model::run::outcome::{Change, ChangeSpec, Child, Children, Declared, Field, OutcomeSpec, Verdict};
use temper_agent_model::run::{self, Budget};
use temper_agent_model::session;
use temper_agent_model::tools;
use temper_agent_model::{Event, Fact, Request};
use temper_lib::{Duration, ReplyTo, Time, Token};
use temper_worker_model_agent::channel::{Ask, Down, Finish, Push, Reply, RunFailure, Up};

/// What the agent's protocol layer holds for an agent process's channel.
#[derive(Debug)]
pub struct Link {
    /// The agent model's name for the worker's side of the run, which its
    /// start carries and its answer goes to.
    pub worker: Token,
    /// The repositories of the workspace the process was spawned in, where io
    /// put them: what the protocol layer adds to the charter.
    pub checkout: Checkout,
    /// The run's name, once the agent model has admitted it.
    pub run: Option<Token>,
}

/// Where a request of the agent model goes, as its protocol layer routes it.
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

/// What the agent model hears of what the worker sent `link`'s run: nothing,
/// for what it has no ear for yet.
#[must_use]
pub fn down(down: Down, link: &Link) -> Option<Event> {
    match down {
        Down::Start { charter, snapshot } => {
            assert!(snapshot.is_none(), "the agent never parks, so it is never resumed");
            let charter = self::charter(&charter, link.checkout.clone());
            Some(Event::Start { reply_to: ReplyTo::new(link.worker), worker: link.worker, charter })
        }
        Down::Event { event: _ } => None,
        Down::Answer { call, reply } => Some(answer(call, &reply)),
        // A cancel that crossed a refusal finds no run.
        Down::Cancel => link.run.map(|run| Event::Cancel { run }),
    }
}

/// The agent model's terminal for its push `call`, for the worker's answer.
#[must_use]
pub fn answer(call: Token, reply: &Reply) -> Event {
    match reply {
        Reply::Pushed(pushed) => Event::Pushed { owner: call, push: push(*pushed) },
        Reply::Unavailable | Reply::Busy | Reply::TooLarge => Event::Pushed { owner: call, push: run::Push::Failed },
        Reply::Withdrawn => Event::HostCancelled { owner: call },
        Reply::Relayed { answer: _ } => panic!("the run relays no calls, so no relayed answer comes down"),
    }
}

/// The run's push for the worker's.
#[must_use]
pub fn push(push: Push) -> run::Push {
    match push {
        Push::Done => run::Push::Done,
        Push::Moved => run::Push::Moved,
        Push::Failed | Push::Nothing => run::Push::Failed,
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
        run::Failure::Model(_) => RunFailure::Model,
        run::Failure::Budget(_) => RunFailure::Budget,
        run::Failure::Policy(_) => RunFailure::Policy,
        run::Failure::Cancelled => RunFailure::Cancelled,
        run::Failure::Stale => RunFailure::Stale,
    }
}

/// A push's commit message: the change's title, then its body after a blank
/// line, if it has one.
fn message(change: &Change) -> Box<[u8]> {
    let Change { title, body } = change;
    let mut message = title.to_vec();
    if !body.is_empty() {
        message.extend_from_slice(b"\n\n");
        message.extend_from_slice(body);
    }
    message.into()
}

/// What goes up for `fact`.
#[must_use]
pub fn fact(fact: Fact) -> Up {
    let kind = match fact {
        Fact::Run { fact } => run_fact(fact),
        Fact::Session { fact } => Some(session_fact(fact)),
    };
    match kind {
        Some(kind) => Up::Fact { fact: kind.into() },
        None => Up::LongDone,
    }
}

/// The kind of a run's fact; none for the end of a check, which is told as
/// such.
fn run_fact(fact: run_facts::Fact) -> Option<&'static [u8]> {
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

/// The run's charter for the engine's, encoded as the fake engine encodes it
/// (`temper_fake_engine_model::charter`: fields in declared order; a `bool`
/// one byte, 0 or 1; a `u32` or `u64` little-endian, a time a `u64` of
/// nanoseconds; bytes a `u32` length, then the bytes; a list a `u32` count,
/// then its items), with `checkout`'s repositories. Every byte is read.
#[must_use]
pub fn charter(bytes: &[u8], checkout: Checkout) -> run::Charter {
    let mut reader = Reader { bytes };
    let brief = reader.bytes();
    let tools = charter::Tools { inspect: reader.flag(), modify: reader.flag(), shell: reader.flag() };
    let forge = reader.flag();
    let agents = reader.flag();
    let outlets = reader.names().into_iter().map(|name| charter::Outlet { name }).collect();
    let change = reader.flag();
    let checks = reader.flag();
    let verdicts = (0..reader.word()).map(|_| reader.verdict()).collect();
    let budget = Budget {
        turns: reader.word(),
        input: reader.long(),
        output: reader.long(),
        cache_read: reader.long(),
        cache_write: reader.long(),
        time: Duration::from_nanos(reader.long()),
    };
    let endpoint = charter::Endpoint(reader.word());
    let model = reader.bytes();
    let max_tokens = reader.word();
    let models = reader.names().into_iter().map(|model| charter::Llm { endpoint, model, max_tokens }).collect();
    reader.end();
    run::Charter {
        brief,
        checkout,
        grants: charter::Grants { tools, forge, agents, outlets },
        outcome: OutcomeSpec { change: change.then_some(ChangeSpec { checks }), verdicts },
        budget,
        llm: charter::Llm { endpoint, model, max_tokens },
        models,
    }
}

/// `declared`, encoded for the engine as a charter is: a byte for which
/// outcome it is, 0 for a change and 1 for a verdict, then its fields in
/// declared order, and likewise for a verdict's children and their fields.
#[must_use]
pub fn outcome(declared: &Declared) -> Box<[u8]> {
    let mut writer = Writer::default();
    match declared {
        Declared::Change(Change { title, body }) => {
            writer.byte(0);
            writer.bytes(title);
            writer.bytes(body);
        }
        Declared::Verdict(Verdict { name, body, children }) => {
            writer.byte(1);
            writer.bytes(name);
            writer.bytes(body);
            writer.count(children.len());
            for Child { kind, fields } in children {
                writer.bytes(kind);
                writer.count(fields.len());
                for Field { name, value } in fields {
                    writer.bytes(name);
                    writer.bytes(value);
                }
            }
        }
    }
    writer.bytes.into()
}

/// The outcome `outcome` encodes, as the engine side reads it back. Every
/// byte is read.
#[must_use]
pub fn declared(outcome: &[u8]) -> Declared {
    let mut reader = Reader { bytes: outcome };
    let declared = match reader.byte() {
        0 => Declared::Change(Change { title: reader.bytes(), body: reader.bytes() }),
        1 => {
            let name = reader.bytes();
            let body = reader.bytes();
            let children = (0..reader.word()).map(|_| reader.child()).collect();
            Declared::Verdict(Verdict { name, body, children })
        }
        other => panic!("an outcome is a change (0) or a verdict (1), not {other}"),
    };
    reader.end();
    declared
}

/// Reads an encoding front to back.
#[derive(Debug)]
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn verdict(&mut self) -> VerdictRule {
        let name = self.bytes();
        let children = Children { min: self.word(), max: self.word() };
        VerdictRule { name, children, kinds: self.names(), fields: self.names() }
    }

    fn child(&mut self) -> Child {
        let kind = self.bytes();
        let fields = (0..self.word()).map(|_| Field { name: self.bytes(), value: self.bytes() }).collect();
        Child { kind, fields }
    }

    fn names(&mut self) -> Box<[Box<[u8]>]> {
        (0..self.word()).map(|_| self.bytes()).collect()
    }

    fn bytes(&mut self) -> Box<[u8]> {
        let len = usize::try_from(self.word()).expect("a u32 fits in a usize");
        self.raw(len).into()
    }

    fn flag(&mut self) -> bool {
        match self.byte() {
            0 => false,
            1 => true,
            other => panic!("a flag is 0 or 1, not {other}"),
        }
    }

    fn byte(&mut self) -> u8 {
        let [byte] = self.raw(1) else { unreachable!("one byte was taken") };
        *byte
    }

    fn word(&mut self) -> u32 {
        u32::from_le_bytes(self.raw(4).try_into().expect("four bytes were taken"))
    }

    fn long(&mut self) -> u64 {
        u64::from_le_bytes(self.raw(8).try_into().expect("eight bytes were taken"))
    }

    fn raw(&mut self, len: usize) -> &'a [u8] {
        let (taken, rest) = self.bytes.split_at_checked(len).expect("an encoding holds what it counts");
        self.bytes = rest;
        taken
    }

    fn end(&self) {
        assert!(self.bytes.is_empty(), "an encoding is all its bytes, but {} are left", self.bytes.len());
    }
}

/// Writes an encoding front to back.
#[derive(Default, Debug)]
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn bytes(&mut self, bytes: &[u8]) {
        self.count(bytes.len());
        self.bytes.extend_from_slice(bytes);
    }

    fn count(&mut self, count: usize) {
        let count = u32::try_from(count).expect("an outcome's counts fit a u32");
        self.bytes.extend_from_slice(&count.to_le_bytes());
    }

    fn byte(&mut self, byte: u8) {
        self.bytes.push(byte);
    }
}
