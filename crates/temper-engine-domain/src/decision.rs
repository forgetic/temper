//! The root's ordered commit seam (domain/engine.md, sections 5.1–5.2).
//! [`Decision`] keeps a bounded unique-key write set and owned deliveries;
//! [`Journal`] keeps deployment counters, cumulative durability and deliveries
//! held behind their prerequisite commits. It never knows child policy, worker
//! placement, store encoding or IO; the walking root routes internal deliveries.
//!
//! Call [`takes`] before child mutation, then [`accept`] once synchronous routes
//! finish. The store answers issued commits through [`committed`] or
//! [`uncommitted`]; [`resume`] releases at most one ready delivery. Refused
//! admission returns ownership, successful admission may emit one commit, and
//! failed storage emits one stop notice. No helper waits for its own effect.
use crate::{Deployment, Family, Key, Record, Write};
use alloc::boxed::Box;
use skein_lib::{List, Queue, ReplyTo, Token};
use temper_engine_domain_people as people;

/// Startup capacities supplied by the root, immutable across journal calls
/// (domain/engine.md, 5.1–5.2). Positive commit/write/delivery room and a held
/// queue at least as large as one delivery batch are required.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Maximum issued commits not yet cumulatively answered; saturation refuses
    /// a new decision before children change (domain/engine.md, 5.1).
    pub commits: u32,
    /// Delivery slots retained across decisions until durability permits release;
    /// the walking root reserves additional callback batches (domain/engine.md, section 5.2).
    pub held: u32,
    /// Maximum unique writes per commit, including its automatically saved
    /// deployment header; a decision has one fewer slot (domain/engine.md, 5.1 and 5.4).
    pub writes: u32,
    /// Maximum owned deliveries in one decision; admission requires this much
    /// held room before routing (domain/engine.md, 5.2).
    pub deliveries: u32,
    /// Maximum transcript bytes or deep owned bytes in a wrapped child row;
    /// excess writes return ownership (domain/engine.md, 5.3, 5.6 and 7.2).
    pub transcript_bytes: u32,
    /// Maximum owned result text or assignment section text admitted to a held
    /// delivery (domain/engine.md, 5.2, 7.1 and 7.4).
    pub result_bytes: u32,
}

/// Owned root effects held after the decision they follow (domain/engine.md, section 5.2). External notices have no acknowledgement of their own; web
/// replies consume one reply destination. Internal fleet/load deliveries are
/// resumed by the root and never passed to the protocol as child events.
#[derive(PartialEq, Eq, Debug)]
pub enum Delivery {
    /// Root-to-authenticated named reader: one current durable held-chat view,
    /// derived from tasks without a persistent people inbox.
    EscalationReply {
        /// One web-issued reply right.
        to: ReplyTo,
        /// Authenticated person permitted to see this held chat.
        person: u64,
        /// Bounded semantic view; rejection owns at most `result_bytes`
        context: Box<temper_engine_domain_tasks::EscalationContext>,
    },
    /// Root internal historical read, released after preceding commit; consumed
    /// once by root and never exposed to the shell.
    ReadEscalationDecision {
        /// Reserved root read slot, retired after IO terminal.
        waiter: Token,
    },
    /// Journal caller to web: one people terminal without an attached session;
    /// the walking root uses `WebReply` instead (domain/people.md, 11).
    Reply {
        /// Web-issued right to exactly this terminal; moved once (domain/people.md, 11).
        to: ReplyTo,
        /// Typed, secret-free reply with no owned unbounded body (domain/people.md, 11).
        reply: people::Reply,
    },
    /// Root to worker: accepted turn is durable; release its retained body.
    /// This notice has no terminal event (domain/engine.md, 7.2).
    AcknowledgeTurn {
        /// Worker protocol destination echoed through fleet (domain/worker.md, 2).
        channel: Token,
        /// Durable task whose turn was accepted (domain/engine.md, 7.2).
        task: u64,
        /// Root-issued activation fence for that turn (domain/engine.md, 7.2).
        attempt: u64,
        /// Positive accepted turn number, represented by `u32` (domain/engine.md, 7.2).
        turn: u32,
    },
    /// Root to worker: the attempt terminal is durable; release its answer.
    /// This notice has no terminal event (domain/engine.md, 7.4).
    Acknowledge {
        /// Worker protocol destination echoed through fleet (domain/worker.md, 2).
        channel: Token,
        /// Durable task whose attempt answered (domain/engine.md, 7.4).
        task: u64,
        /// Root-issued attempt whose answer may be forgotten (domain/engine.md, 7.4).
        attempt: u64,
    },
    /// Root to worker: stop this attempt. Its answer or lost-channel grace
    /// outcome returns through fleet; this notice allocates no additional
    /// attempt (domain/engine.md, 7.4; domain/worker.md, 2).
    Cancel {
        /// Worker protocol destination echoed through fleet (domain/worker.md, 2).
        channel: Token,
        /// Task owning the attempt being cancelled (domain/engine.md, 7.4).
        task: u64,
        /// Root-issued activation to cancel, not a later attempt (domain/engine.md, 7.4).
        attempt: u64,
    },
    /// Root to fleet after durability; the root resumes it internally (domain/engine.md, 5.2).
    Fleet(
        /// Root-issued `Start`, `TurnKept`, `Acknowledge` or `Cancel` callback.
        /// `Start` workstream bytes are at most journal `transcript_bytes`; other
        /// payloads are fixed-size. Fleet start/cancel consequences route back
        /// inside the root (domain/engine.md, section 5.2).
        temper_engine_domain_fleet::Event,
    ),
    /// Root to worker: complete bounded assignment after its claim commit (domain/engine.md, 7.1).
    Assigned {
        /// Worker protocol destination, bounded by fleet worker room (domain/worker.md, 2).
        channel: Token,
        /// Complete claim/brief/grant; section slots and owned bytes validated at journal admission (domain/engine.md, 7.1 and 9).
        assignment: crate::engine::Assignment,
    },
    /// Root to web: one terminal reply, optionally carrying its sign-in candidate;
    /// only a successful people reply admits that session (domain/people.md, 3 and 11).
    WebReply {
        /// Web-issued right to this one terminal reply (domain/people.md, 11).
        to: ReplyTo,
        /// Root-issued sign-in candidate accompanying a sign-in decision, or none.
        /// A refusal can leave it unused; clients require a successful `SignedIn`
        /// reply before using it (domain/people.md, 3; domain/engine.md, 5.4).
        sign_in: Option<u64>,
        /// Fixed-size secret-free people terminal, held after the decision it
        /// names; owns no display or credential bytes (domain/people.md, 5.1 and 11).
        reply: people::Reply,
    },
    /// Root to worker: refuse its hello because of startup, shape or fleet
    /// admission; no attempt is assigned and no terminal is owed for this notice
    /// (domain/worker.md, 2).
    Refuse {
        /// Fixed-size worker protocol destination echoed from its refused hello;
        /// no worker slot is admitted (domain/worker.md, 2).
        channel: Token,
    },
    /// Root resumes authenticated historical result IO only after prior commits (domain/engine.md, 5.3).
    ReadResult {
        /// Root-owned load waiter identity; internal routing consumes this notice once (domain/engine.md, 5.3).
        waiter: Token,
    },
    /// Root to worker: fleet lacked turn admission room; keep the body and
    /// retry after a backoff, without charging (domain/worker.md, 2).
    TurnBusy {
        /// Current worker protocol channel (domain/worker.md, 2).
        channel: Token,
        /// Durable task/claim identity (domain/engine.md, 7.2).
        task: u64,
        /// Activation fence checked by fleet (domain/engine.md, 7.2).
        attempt: u64,
        /// Positive turn retained by the sender for retry (domain/engine.md, 7.2).
        turn: u32,
    },
    /// Root to store after its prerequisite commits, resumed internally (domain/engine.md, 5.3).
    Load {
        /// Root-owned load waiter identity; internal routing consumes this notice once (domain/engine.md, 5.3).
        waiter: Token,
        /// Closed key range, validated by the root load child (domain/engine.md, 5.3).
        range: crate::Range,
        /// Exclusive validated page continuation, or range beginning (domain/engine.md, 5.3).
        after: Option<Key>,
    },
    /// Root to authenticated web reader: terminal derived from an ended task (domain/people.md, 6).
    ResultReply {
        /// Web-issued right to this one terminal reply (domain/people.md, 11).
        to: ReplyTo,
        /// Authenticated requester number from people session and ended task (domain/people.md, 6).
        person: u64,
        /// Durable ended task number naming this result, never a separate inbox record (domain/people.md, 6).
        task: u64,
        /// Owned result text bounded by journal `result_bytes` (domain/people.md, 6).
        words: Box<[u8]>,
    },
    /// Root to the task requester: live notice of its durable ending, with no
    /// reply right or persistent people inbox; historical reads use `ResultReply`
    /// (domain/people.md, section 6).
    Result {
        /// Durable person requester, checked by the task route (domain/people.md, 6).
        person: u64,
        /// Ended task whose record remains the source of this result (domain/people.md, 6).
        task: u64,
        /// Owned result text, at most journal `result_bytes` (domain/engine.md, 5.2).
        words: Box<[u8]>,
    },
}

/// Journal to its root caller: at most one value per entry point; the caller
/// translates it to store IO or an outward/internal delivery (domain/engine.md, 5).
#[derive(PartialEq, Eq, Debug)]
pub enum Output {
    /// Ordered atomic store transaction, ended by `committed` or `uncommitted`
    /// using the issued number (domain/engine.md, 5.1).
    Commit {
        /// Positive monotonically allocated commit identity (domain/engine.md, 5.1).
        number: u64,
        /// Owned unique-key writes including the header, at most `Limits::writes`
        /// (domain/engine.md, 5.1 and 5.4).
        writes: Box<[Write]>,
    },
    /// One effect whose prerequisite commit is durable (domain/engine.md, 5.2).
    Deliver(/** Owned bounded effect, consumed once by the root (domain/engine.md, 5.2). */ Delivery),
    /// Storage failed; the caller stops the process without releasing later work.
    /// This notice has no terminal event (domain/engine.md, 5.1).
    Stop,
}

/// One root route's bounded transient writes and deliveries; it owns their
/// payloads until accepted or returned on refusal (domain/engine.md, 5.1–5.2).
/// This value knows no durability and emits nothing on drop.
#[derive(Debug)]
pub struct Decision {
    limits: Limits,
    writes: List<Write>,
    deliveries: Queue<Delivery>,
}

#[derive(Debug)]
struct Held {
    after: u64,
    delivery: Delivery,
}

/// Root-owned deployment counters and ordered durability barrier
/// (domain/engine.md, 5.1–5.4). It retains bounded delivery ownership, never
/// store IO handles or a second copy of child state.
#[derive(Debug)]
pub struct Journal {
    limits: Limits,
    deployment: Deployment,
    durable: u64,
    dirty: bool,
    stopped: bool,
    held: Queue<Held>,
}

impl Decision {
    /// Root-only preflight of unused write/delivery slots before a synchronous
    /// role replacement; no reservation can interleave with another mutation
    pub(crate) fn room_for(&self, writes: u32, deliveries: u32) -> bool {
        self.writes.room() >= writes && self.deliveries.room() >= deliveries
    }

    /// Allocate one transient decision from validated startup limits; no effect
    /// is issued until `accept` (domain/engine.md, 5.1–5.2).
    #[must_use]
    pub fn new(limits: &Limits) -> Decision {
        assert!(worst_case(limits).is_some(), "valid root journal limits");
        Decision {
            limits: *limits,
            writes: List::with_capacity(limits.writes.checked_sub(1).expect("header slot reserved")),
            deliveries: Queue::with_capacity(limits.deliveries),
        }
    }

    /// A replacement at the same key keeps its original position. All child
    /// callbacks finish before submission, so only the final value is saved.
    /// The root supplies the write; successful admission emits no event. Bounds
    /// or capacity refusal returns it unchanged. Deployment writes are reserved
    /// for `accept`, not callers (domain/engine.md, 5.1 and 5.6).
    pub fn write(&mut self, limits: &Limits, write: Write) -> Result<(), Write> {
        assert!(*limits == self.limits, "decision uses its configured limits");
        let within = match &write {
            Write::Save(Record::Deployment(_)) | Write::Erase(Key::Deployment | Key::EscalationDecision { .. }) => {
                false
            }
            Write::Save(Record::Turn(row)) => {
                row.task != 0
                    && row.attempt != 0
                    && row.turn != 0
                    && row.transcript.len() <= usize::try_from(limits.transcript_bytes).expect("u32 fits usize")
            }
            Write::Erase(
                Key::Turn { .. } | Key::RunProof { .. } | Key::Terminal { .. } | Key::Tasks(_) | Key::People(_),
            ) => true,
            Write::Save(Record::EscalationDecision(row)) => {
                row.task != 0
                    && row.revision != 0
                    && row.by != 0
                    && row.requester != 0
                    && match &row.decision {
                        temper_engine_domain_people::EscalationDecision::Release
                        | temper_engine_domain_people::EscalationDecision::Pass => true,
                        temper_engine_domain_people::EscalationDecision::Reject { reason } => {
                            reason.len()
                                <= usize::try_from(limits.result_bytes.min(limits.transcript_bytes))
                                    .expect("u32 fits usize")
                        }
                    }
            }
            Write::Save(Record::Tasks(_) | Record::People(_) | Record::RunProof(_) | Record::Terminal(_)) => {
                match crate::store::owned_bytes(&write) {
                    Some(bytes) => bytes <= u64::from(limits.transcript_bytes),
                    None => false,
                }
            }
        };
        if !within {
            return Err(write);
        }
        let key = write.key();
        for at in 0..self.writes.len() {
            let previous = self.writes.get_mut(at).expect("bounded write index");
            if previous.key() == key {
                *previous = write;
                return Ok(());
            }
        }
        self.writes.push(write)
    }

    /// Retain one root effect for the decision, without releasing or issuing it.
    /// Bounds or capacity refusal returns ownership; successful admission ends
    /// through journal release after durability (domain/engine.md, section 5.2).
    pub fn deliver(&mut self, limits: &Limits, delivery: Delivery) -> Result<(), Delivery> {
        assert!(*limits == self.limits, "decision uses its configured limits");
        let within = match &delivery {
            Delivery::Result { words, .. } | Delivery::ResultReply { words, .. } => {
                words.len() <= usize::try_from(limits.result_bytes).expect("u32 fits usize")
            }
            Delivery::EscalationReply { context, .. } => {
                context.task != 0
                    && context.requester != 0
                    && match &context.escalation {
                        temper_engine_domain_tasks::Escalation::Rejected { revision, by, reason } => {
                            *revision != 0
                                && *by != 0
                                && reason.len() <= usize::try_from(limits.result_bytes).expect("u32 fits usize")
                        }
                        temper_engine_domain_tasks::Escalation::Waiting { revision, holder } => {
                            *revision != 0
                                && match holder {
                                    temper_engine_domain_tasks::EscalationHolder::Person(person) => {
                                        *person != 0 && *person == context.requester
                                    }
                                    temper_engine_domain_tasks::EscalationHolder::Role { project, .. } => {
                                        *project == context.project
                                    }
                                }
                        }
                        temper_engine_domain_tasks::Escalation::Unheld { .. }
                        | temper_engine_domain_tasks::Escalation::Routing { .. } => false,
                    }
            }
            Delivery::Fleet(event) => fleet_delivery_within(event, limits),
            Delivery::Assigned { assignment, .. } => assignment_within(assignment, limits),
            Delivery::Reply { .. }
            | Delivery::AcknowledgeTurn { .. }
            | Delivery::Acknowledge { .. }
            | Delivery::Cancel { .. }
            | Delivery::WebReply { .. }
            | Delivery::Refuse { .. }
            | Delivery::ReadEscalationDecision { .. }
            | Delivery::ReadResult { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Load { .. } => true,
        };
        if !within {
            return Err(delivery);
        }
        self.deliveries.try_push(delivery)
    }
}

impl Journal {
    /// An empty store needs the deployment identity committed even before its
    /// first task. The id is a root input, drawn once by the shell at startup.
    /// Validated limits size the fixed held queue; the next accepted decision
    /// emits the dirty header's commit (domain/engine.md, section 5.4).
    #[must_use]
    pub fn bootstrap(id: [u8; 16], limits: &Limits) -> Journal {
        let deployment =
            Deployment { id, tasks: 0, people: 0, sign_ins: 0, messages: 0, runs: 0, calls: 0, commits: 0 };
        let mut journal = Journal::new(deployment, limits);
        journal.dirty = true;
        journal
    }

    /// A loaded header names the last fully applied commit. It is durable;
    /// outstanding answers from a previous process need not be reconstructed.
    /// The root supplies the decoded store header and validated startup bounds;
    /// construction emits no effect (domain/engine.md, 5.4 and 6).
    #[must_use]
    pub fn new(deployment: Deployment, limits: &Limits) -> Journal {
        assert!(worst_case(limits).is_some(), "valid root journal limits");
        Journal {
            limits: *limits,
            durable: deployment.commits,
            deployment,
            dirty: false,
            stopped: false,
            held: Queue::with_capacity(limits.held),
        }
    }

    /// Pure snapshot of allocated counters, which may run ahead of the durable
    /// store until its issued commits answer (domain/engine.md, 5.1 and 5.4).
    #[must_use]
    pub const fn deployment(&self) -> Deployment {
        self.deployment
    }

    /// Pure query for the last cumulatively answered commit; bounded by the
    /// allocated commit counter (domain/engine.md, 5.1).
    #[must_use]
    pub const fn durable(&self) -> u64 {
        self.durable
    }

    /// Pure query for the storage-failure fence; true prevents subsequent
    /// admission and release (domain/engine.md, 5.1).
    #[must_use]
    pub const fn stopped(&self) -> bool {
        self.stopped
    }

    /// Pure shell idle query: no dirty header, held delivery or unanswered
    /// issued commit remains. A stopped journal is not complete
    /// (domain/engine.md, section 5.2).
    #[must_use]
    pub fn quiescent(&self) -> bool {
        !self.stopped && !self.dirty && self.held.is_empty() && self.durable == self.deployment.commits
    }

    /// Pure root query for unused held-delivery slots; callback admission uses
    /// this in addition to `takes` (domain/engine.md, section 5.2).
    pub(crate) fn held_room(&self) -> u32 {
        self.held.room()
    }

    /// Pure query: whether the front held effect can be released by `resume`.
    /// It reports no child or IO readiness (domain/engine.md, 5.2).
    #[must_use]
    pub fn ready(&self) -> bool {
        if self.stopped {
            return false;
        }
        match self.held.iter().next() {
            Some(held) => held.after <= self.durable,
            None => false,
        }
    }
}

/// Called before routing anything that may mutate a child. Reserving the
/// whole decision avoids partially applying a child and discovering pressure.
/// Pure query for one journal batch, with no mutation or emitted terminal; the
/// walking root additionally reserves callback room (domain/engine.md, section 5.1).
#[must_use]
pub fn takes(journal: &Journal, limits: &Limits) -> bool {
    assert!(*limits == journal.limits, "journal uses its configured limits");
    !journal.stopped
        && journal.deployment.commits != u64::MAX
        && journal.deployment.commits.checked_sub(journal.durable).expect("durable never exceeds made")
            < u64::from(limits.commits)
        && journal.held.room() >= limits.deliveries
}

/// Root numbers are never reused. Allocation is part of the admitted
/// decision, even if its candidate is unused; the next commit saves the gap.
/// The root chooses the counter family after admission. Returns its next positive
/// `u64`, or `None` when stopped/exhausted, with no effect on refusal; emits no
/// request itself (domain/engine.md, 5.4).
pub fn fresh(journal: &mut Journal, family: Family) -> Option<u64> {
    if journal.stopped {
        return None;
    }
    let counter = match family {
        Family::Task => &mut journal.deployment.tasks,
        Family::Person => &mut journal.deployment.people,
        Family::SignIn => &mut journal.deployment.sign_ins,
        Family::Message => &mut journal.deployment.messages,
        Family::Run => &mut journal.deployment.runs,
        Family::Call => &mut journal.deployment.calls,
    };
    let next = counter.checked_add(1)?;
    *counter = next;
    journal.dirty = true;
    Some(next)
}

/// Return ownership on refusal. The root must call `takes` before making
/// the decision; this defensive check does not undo already-routed children.
/// Reserve one output. A dirty header or nonempty write set produces one atomic
/// commit, ended by the store; a read-only decision issues none. Every retained
/// delivery follows the latest allocated commit (domain/engine.md, 5.1–5.2).
pub fn accept(
    journal: &mut Journal,
    limits: &Limits,
    mut decision: Decision,
    out: &mut Queue<Output>,
) -> Result<(), Decision> {
    assert!(out.room() >= 1, "one root journal output reserved");
    if !takes(journal, limits)
        || decision.limits != *limits
        || decision.writes.len() >= limits.writes
        || decision.deliveries.len() > limits.deliveries
        || decision.deliveries.len() > journal.held.room()
    {
        return Err(decision);
    }
    let writing = journal.dirty || !decision.writes.is_empty();
    if writing {
        journal.deployment.commits = journal.deployment.commits.checked_add(1).expect("admitted commit number");
        let mut writes = List::with_capacity(decision.writes.len().checked_add(1).expect("reserved header slot"));
        writes.push(Write::Save(Record::Deployment(journal.deployment))).expect("header slot reserved");
        for at in 0..decision.writes.len() {
            let source = decision.writes.get_mut(at).expect("admitted write index");
            let key = source.key();
            // The source list is consumed in this step. An erase is a
            // payload-free terminal placeholder, never submitted again.
            let write = core::mem::replace(source, Write::Erase(key));
            writes.push(write).expect("admitted decision writes");
        }
        out.push(Output::Commit { number: journal.deployment.commits, writes: writes.into_boxed() });
        journal.dirty = false;
    }
    drop(decision.writes);
    for _ in 0..decision.deliveries.len() {
        let delivery = decision.deliveries.pop().expect("admitted delivery count");
        journal.held.push(Held { after: journal.deployment.commits, delivery });
    }
    Ok(())
}

/// A cumulative store answer only makes outputs ready. The ready pass
/// releases one per call; it never drains all of them inside the store step.
/// The store echoes an issued `u64` commit number. Duplicate/older terminals are
/// inert; a number beyond issued commits is a caller error (domain/engine.md, 5.1).
pub fn committed(journal: &mut Journal, number: u64) {
    if journal.stopped || number <= journal.durable {
        return;
    }
    assert!(number <= journal.deployment.commits, "store answers only issued commits");
    journal.durable = number;
}

/// Root ready pass: reserve one output and release at most one durable held
/// effect in FIFO order; stopped or unready journals emit nothing. A delivery
/// is consumed by its root route, not answered to this seam (domain/engine.md, 5.2).
pub fn resume(journal: &mut Journal, out: &mut Queue<Output>) {
    assert!(out.room() >= 1, "one root ready output reserved");
    if journal.ready() {
        let held = journal.held.pop().expect("ready front exists");
        out.push(Output::Deliver(held.delivery));
    }
}

/// Store to journal: an issued commit failed. Reserve one output; the first
/// failure beyond durable progress emits `Stop` and retains all held effects.
/// Older or repeated failure notices are inert (domain/engine.md, 5.1).
pub fn uncommitted(journal: &mut Journal, number: u64, out: &mut Queue<Output>) {
    assert!(out.room() >= 1, "one root stop output reserved");
    if journal.stopped || number <= journal.durable {
        return;
    }
    assert!(number <= journal.deployment.commits, "failure names an issued commit");
    journal.stopped = true;
    // Nothing is released, including outputs tagged with a later commit.
    out.push(Output::Stop);
}

/// Pure startup heap calculation for journal containers and simultaneously
/// owned decision/commit/held payloads, excluding allocator overhead. Invalid
/// bounds or arithmetic overflow return `None`; no state or effect is created
/// (domain/engine.md, 4 and 5.1–5.3).
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.commits == 0 || limits.writes == 0 || limits.held < limits.deliveries || limits.deliveries == 0 {
        return None;
    }
    Queue::<Held>::worst_case(limits.held)?
        .checked_add(List::<Write>::worst_case(limits.writes)?.checked_mul(2)?)?
        .checked_add(Queue::<Delivery>::worst_case(limits.deliveries)?)?
        .checked_add(u64::from(limits.writes).checked_mul(2)?.checked_mul(u64::from(limits.transcript_bytes))?)?
        .checked_add(
            u64::from(limits.held).checked_add(u64::from(limits.deliveries))?.checked_mul(
                u64::from(limits.result_bytes).max(u64::from(limits.transcript_bytes)).checked_add(
                    List::<temper_engine_domain_brief::Section>::worst_case(limits.deliveries)?
                        .max(u64::try_from(size_of::<temper_engine_domain_tasks::EscalationContext>()).ok()?),
                )?,
            )?,
        )
}

fn fleet_delivery_within(event: &temper_engine_domain_fleet::Event, limits: &Limits) -> bool {
    use temper_engine_domain_fleet::Event;
    match event {
        Event::Start { workstream, .. } => {
            workstream.len() <= usize::try_from(limits.transcript_bytes).expect("u32 fits usize")
        }
        Event::TurnKept { .. } | Event::Acknowledge { .. } | Event::Cancel { .. } => true,
        Event::Adopt { .. }
        | Event::Inbound { .. }
        | Event::Relayed { .. }
        | Event::Loaded
        | Event::Grant { .. }
        | Event::Rejected { .. }
        | Event::Exhausted { .. }
        | Event::Hello { .. }
        | Event::Lost { .. }
        | Event::Answer { .. }
        | Event::Turn { .. }
        | Event::TurnBusy { .. }
        | Event::Relay { .. }
        | Event::Bounced { .. }
        | Event::Told { .. } => false,
    }
}

fn assignment_within(assignment: &crate::engine::Assignment, limits: &Limits) -> bool {
    if assignment.task == 0
        || assignment.attempt == 0
        || assignment.sections.len() > usize::try_from(limits.deliveries).expect("u32 fits usize")
    {
        return false;
    }
    let mut owned = 0_u64;
    for section in &assignment.sections {
        let bytes = match &section.body {
            temper_engine_domain_brief::Body::Text(bytes) => u64::try_from(bytes.len()).expect("usize fits u64"),
            temper_engine_domain_brief::Body::Missing(_) => 0,
        };
        let Some(total) = owned.checked_add(bytes) else {
            return false;
        };
        owned = total;
    }
    owned <= u64::from(limits.result_bytes)
}
