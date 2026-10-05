//! One decision, one ordered commit, bounded release (domain/engine.md, 5).
use crate::{Deployment, Family, Key, Record, Write};
use alloc::boxed::Box;
use skein_lib::{List, Queue, ReplyTo};
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
    Reply { to: ReplyTo, reply: people::Reply },
    AcknowledgeTurn { channel: u64, task: u64, attempt: u64, turn: u32 },
    Acknowledge { channel: u64, task: u64, attempt: u64 },
    Cancel { channel: u64, task: u64, attempt: u64 },
    Result { person: u64, task: u64, words: Box<[u8]> },
}
#[derive(PartialEq, Eq, Debug)]
pub enum Output {
    Commit { number: u64, writes: Box<[Write]> },
    Deliver(Delivery),
    Stop,
}
#[derive(Debug)]
pub struct Decision {
    limits: Limits,
    writes: List<Write>,
    deliveries: List<Delivery>,
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
    pub fn new(l: &Limits) -> Decision {
        assert!(worst_case(l).is_some(), "valid root journal limits");
        Decision {
            limits: *l,
            writes: List::with_capacity(l.writes.checked_sub(1).expect("header slot reserved")),
            deliveries: List::with_capacity(l.deliveries),
        }
    }
    /// A replacement at the same key keeps its original position. All child
    /// callbacks finish before submission, so only the final value is saved.
    pub fn write(&mut self, l: &Limits, write: Write) -> Result<(), Write> {
        assert!(*l == self.limits, "decision uses its configured limits");
        let within = match &write {
            Write::Save(Record::Deployment(_)) | Write::Erase(Key::Deployment) => false,
            Write::Save(Record::Turn(row)) => {
                row.task != 0
                    && row.attempt != 0
                    && row.turn != 0
                    && row.transcript.len() <= usize::try_from(l.transcript_bytes).expect("u32 fits usize")
            }
            Write::Erase(Key::Turn { .. }) => true,
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
    pub fn deliver(&mut self, l: &Limits, delivery: Delivery) -> Result<(), Delivery> {
        assert!(*l == self.limits, "decision uses its configured limits");
        let within = match &delivery {
            Delivery::Result { words, .. } => words.len() <= usize::try_from(l.result_bytes).expect("u32 fits usize"),
            Delivery::Reply { .. }
            | Delivery::AcknowledgeTurn { .. }
            | Delivery::Acknowledge { .. }
            | Delivery::Cancel { .. } => true,
        };
        if !within {
            return Err(delivery);
        }
        self.deliveries.push(delivery)
    }
}
impl Journal {
    /// An empty store needs the deployment identity committed even before its
    /// first task. The id is a root input, drawn once by the shell at startup.
    #[must_use]
    pub fn bootstrap(id: [u8; 16], l: &Limits) -> Journal {
        let deployment =
            Deployment { id, tasks: 0, people: 0, sign_ins: 0, messages: 0, runs: 0, calls: 0, commits: 0 };
        let mut journal = Journal::new(deployment, l);
        journal.dirty = true;
        journal
    }
    /// A loaded header names the last fully applied commit. It is durable;
    /// outstanding answers from a previous process need not be reconstructed.
    #[must_use]
    pub fn new(deployment: Deployment, l: &Limits) -> Journal {
        assert!(worst_case(l).is_some(), "valid root journal limits");
        Journal {
            limits: *l,
            durable: deployment.commits,
            deployment,
            dirty: false,
            stopped: false,
            held: Queue::with_capacity(l.held),
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
pub fn takes(j: &Journal, l: &Limits) -> bool {
    assert!(*l == j.limits, "journal uses its configured limits");
    !j.stopped
        && j.deployment.commits != u64::MAX
        && j.deployment.commits.checked_sub(j.durable).expect("durable never exceeds made") < u64::from(l.commits)
        && j.held.room() >= l.deliveries
}
/// Root numbers are never reused. Allocation is part of the admitted
/// decision, even if its candidate is unused; the next commit saves the gap.
pub fn fresh(j: &mut Journal, family: Family) -> Option<u64> {
    if j.stopped {
        return None;
    }
    let counter = match family {
        Family::Task => &mut j.deployment.tasks,
        Family::Person => &mut j.deployment.people,
        Family::SignIn => &mut j.deployment.sign_ins,
        Family::Message => &mut j.deployment.messages,
        Family::Run => &mut j.deployment.runs,
        Family::Call => &mut j.deployment.calls,
    };
    let next = counter.checked_add(1)?;
    *counter = next;
    j.dirty = true;
    Some(next)
}
/// Return ownership on refusal. The root must call `takes` before making
/// the decision; this defensive check does not undo already-routed children.
pub fn accept(j: &mut Journal, l: &Limits, decision: Decision, out: &mut Queue<Output>) -> Result<(), Decision> {
    assert!(out.room() >= 1, "one root journal output reserved");
    if !takes(j, l)
        || decision.limits != *l
        || decision.writes.len() >= l.writes
        || decision.deliveries.len() > l.deliveries
        || decision.deliveries.len() > j.held.room()
    {
        return Err(decision);
    }
    let writing = j.dirty || !decision.writes.is_empty();
    if writing {
        j.deployment.commits = j.deployment.commits.checked_add(1).expect("admitted commit number");
        let mut writes = List::with_capacity(decision.writes.len().checked_add(1).expect("reserved header slot"));
        writes.push(Write::Save(Record::Deployment(j.deployment))).expect("header slot reserved");
        for write in decision.writes.into_boxed() {
            writes.push(write).expect("admitted decision writes");
        }
        out.push(Output::Commit { number: j.deployment.commits, writes: writes.into_boxed() });
        j.dirty = false;
    }
    for delivery in decision.deliveries.into_boxed() {
        j.held.push(Held { after: j.deployment.commits, delivery });
    }
    Ok(())
}
/// A cumulative store answer only makes outputs ready. The ready pass
/// releases one per call; it never drains all of them inside the store step.
pub fn committed(j: &mut Journal, number: u64) {
    if j.stopped || number <= j.durable {
        return;
    }
    assert!(number <= j.deployment.commits, "store answers only issued commits");
    j.durable = number;
}
pub fn resume(j: &mut Journal, out: &mut Queue<Output>) {
    assert!(out.room() >= 1, "one root ready output reserved");
    if j.ready() {
        let held = j.held.pop().expect("ready front exists");
        out.push(Output::Deliver(held.delivery));
    }
}
pub fn uncommitted(j: &mut Journal, number: u64, out: &mut Queue<Output>) {
    assert!(out.room() >= 1, "one root stop output reserved");
    if j.stopped || number <= j.durable {
        return;
    }
    assert!(number <= j.deployment.commits, "failure names an issued commit");
    j.stopped = true;
    // Nothing is released, including outputs tagged with a later commit.
    out.push(Output::Stop);
}
#[must_use]
pub fn worst_case(l: &Limits) -> Option<u64> {
    if l.commits == 0 || l.writes == 0 || l.held < l.deliveries || l.deliveries == 0 {
        return None;
    }
    Queue::<Held>::worst_case(l.held)?
        .checked_add(List::<Write>::worst_case(l.writes)?.checked_mul(2)?)?
        .checked_add(List::<Delivery>::worst_case(l.deliveries)?)?
        .checked_add(u64::from(l.writes).checked_mul(u64::from(l.transcript_bytes))?)?
        .checked_add(u64::from(l.held).checked_add(u64::from(l.deliveries))?.checked_mul(u64::from(l.result_bytes))?)
}
