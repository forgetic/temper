//! One decision, one ordered commit, bounded release (domain/engine.md, 5).
use crate::{Deployment, Family, Key, Record, Write};
use alloc::boxed::Box;
use skein_lib::{List, Queue, ReplyTo, Token};
use temper_engine_domain_people as people;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub commits: u32,
    pub held: u32,
    /// Includes the automatically saved deployment header.
    pub writes: u32,
    pub deliveries: u32,
    pub transcript_bytes: u32,
    pub result_bytes: u32,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Delivery {
    Reply {
        to: ReplyTo,
        reply: people::Reply,
    },
    AcknowledgeTurn {
        channel: Token,
        task: u64,
        attempt: u64,
        turn: u32,
    },
    Acknowledge {
        channel: Token,
        task: u64,
        attempt: u64,
    },
    Cancel {
        channel: Token,
        task: u64,
        attempt: u64,
    },
    /// Root to fleet after durability; the root resumes it internally (domain/engine.md, 5.2).
    Fleet(
        /// Only bounded `Start`, `TurnKept`, `Acknowledge` and `Cancel` callbacks are admitted (domain/engine.md, 5.2).
        temper_engine_domain_fleet::Event,
    ),
    /// Root to worker: complete bounded assignment after its claim commit (domain/engine.md, 7.1).
    Assigned {
        /// Worker protocol destination, bounded by fleet worker room (domain/worker.md, 2).
        channel: Token,
        /// Complete claim/brief/grant; section slots and owned bytes validated at journal admission (domain/engine.md, 7.1 and 9).
        assignment: crate::engine::Assignment,
    },
    /// Root to web: terminal reply, with newly allocated session when signing in (domain/people.md, 3).
    WebReply {
        /// Web-issued right to this one terminal reply (domain/people.md, 11).
        to: ReplyTo,
        /// New root-issued session on a sign-in reply; absent for other replies (domain/people.md, 3).
        sign_in: Option<u64>,
        /// Secret-free people terminal, held after the decision it names (domain/people.md, 5.1).
        reply: people::Reply,
    },
    /// Root to worker: the hello exceeded bounded fleet room (domain/worker.md, 2).
    Refuse {
        /// Worker protocol destination, bounded by fleet worker room (domain/worker.md, 2).
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
    Result {
        person: u64,
        task: u64,
        words: Box<[u8]>,
    },
}

#[derive(PartialEq, Eq, Debug)]
pub enum Output {
    Commit { number: u64, writes: Box<[Write]> ,},
    Deliver(Delivery),
    Stop,
}

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
    pub fn write(&mut self, limits: &Limits, write: Write) -> Result<(), Write> {
        assert!(*limits == self.limits, "decision uses its configured limits");
        let within = match &write {
            Write::Save(Record::Deployment(_)) | Write::Erase(Key::Deployment) => false,
            Write::Save(Record::Turn(row)) => {
                row.task != 0
                    && row.attempt != 0
                    && row.turn != 0
                    && row.transcript.len() <= usize::try_from(limits.transcript_bytes).expect("u32 fits usize")
            }
            Write::Erase(
                Key::Turn { .. } | Key::RunProof { .. } | Key::Terminal { .. } | Key::Tasks(_) | Key::People(_),
            ) => true,
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

    pub fn deliver(&mut self, limits: &Limits, delivery: Delivery) -> Result<(), Delivery> {
        assert!(*limits == self.limits, "decision uses its configured limits");
        let within = match &delivery {
            Delivery::Result { words, .. } | Delivery::ResultReply { words, .. } => {
                words.len() <= usize::try_from(limits.result_bytes).expect("u32 fits usize")
            }
            Delivery::Fleet(event) => fleet_delivery_within(event, limits),
            Delivery::Assigned { assignment, .. } => assignment_within(assignment, limits),
            Delivery::Reply { .. }
            | Delivery::AcknowledgeTurn { .. }
            | Delivery::Acknowledge { .. }
            | Delivery::Cancel { .. }
            | Delivery::WebReply { .. }
            | Delivery::Refuse { .. }
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

    #[must_use]
    pub const fn deployment(&self) -> Deployment {
        self.deployment
    }

    #[must_use]
    pub const fn durable(&self) -> u64 {
        self.durable
    }

    #[must_use]
    pub const fn stopped(&self) -> bool {
        self.stopped
    }

    /// Shell completion fence: no dirty header, held delivery or unanswered
    /// issued commit remains. A stopped journal is not complete
    /// (domain/engine.md, 5.2 and 5.7).
    #[must_use]
    pub fn quiescent(&self) -> bool {
        !self.stopped && !self.dirty && self.held.is_empty() && self.durable == self.deployment.commits
    }

    pub(crate) fn held_room(&self) -> u32 {
        self.held.room()
    }

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
pub fn committed(journal: &mut Journal, number: u64) {
    if journal.stopped || number <= journal.durable {
        return;
    }
    assert!(number <= journal.deployment.commits, "store answers only issued commits");
    journal.durable = number;
}

pub fn resume(journal: &mut Journal, out: &mut Queue<Output>) {
    assert!(out.room() >= 1, "one root ready output reserved");
    if journal.ready() {
        let held = journal.held.pop().expect("ready front exists");
        out.push(Output::Deliver(held.delivery));
    }
}

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

#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.commits == 0 || limits.writes == 0 || limits.held < limits.deliveries || limits.deliveries == 0 {
        return None;
    }
    Queue::<Held>::worst_case(limits.held)?
        .checked_add(List::<Write>::worst_case(limits.writes)?.checked_mul(2)?)?
        .checked_add(Queue::<Delivery>::worst_case(limits.deliveries)?)?
        .checked_add(u64::from(limits.writes).checked_mul(2)?.checked_mul(u64::from(limits.transcript_bytes))?)?
        .checked_add(u64::from(limits.held).checked_add(u64::from(limits.deliveries))?.checked_mul(
            u64::from(limits.result_bytes).max(u64::from(limits.transcript_bytes)).checked_add(List::<
                temper_engine_domain_brief::Section,
            >::worst_case(
                limits.deliveries
            )?)?,
        )?)
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
