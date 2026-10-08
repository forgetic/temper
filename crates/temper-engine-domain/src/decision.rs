//! The root's ordered commit seam (domain/engine.md, sections 5.1–5.2).
//! [`Decision`] keeps a bounded unique-key write set and owned deliveries;
//! skein-lib's [`Journal`] keeps cumulative durability and deliveries held behind
//! their prerequisite commits; [`Counters`] keeps deployment numbers until jig
//! owns them. Neither knows child policy, worker
//! placement, store encoding or IO; the walking root routes internal deliveries.
//!
//! Reserve a decision before child mutation, then call [`accept_pending`] once
//! synchronous routes finish. The store answers issued commits through [`committed`] or
//! [`uncommitted`]; [`resume`] releases at most one ready delivery. Refused
//! admission returns ownership, successful admission may emit one commit, and
//! failed storage emits one stop notice. No helper waits for its own effect.
use crate::{Key, Record, Write};
use alloc::boxed::Box;
use core::mem::size_of;
pub use jig_core::{Counters, fresh};
use jig_core_people as people;
use skein_lib::{
    Decision as SkeinDecision, Journal as SkeinJournal, JournalLimits, JournalRoom, List, Queue, ReplyTo, Token, Wall,
};

/// Startup capacities supplied by the root, immutable across journal calls
/// (domain/engine.md, 5.1–5.2). Positive commit/write/delivery room and a held
/// queue at least as large as one delivery batch are required.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Maximum issued commits not yet cumulatively answered; saturation refuses a new decision
    /// before children change.
    pub commits: u32,
    /// Delivery slots retained across decisions until durability permits release; the walking root
    /// reserves additional callback batches.
    pub held: u32,
    /// Maximum unique writes per commit, including its automatically saved deployment header; a
    /// decision has one fewer slot.
    pub writes: u32,
    /// Maximum owned deliveries in one decision; admission requires this much held room before
    /// routing.
    pub deliveries: u32,
    /// Maximum transcript bytes or deep owned bytes in a wrapped child row; excess writes return
    /// ownership.
    pub transcript_bytes: u32,
    /// Maximum owned result text or assignment section text admitted to a held delivery.
    pub result_bytes: u32,
    /// Maximum deeply owned run charter bytes in a held assignment.
    pub run_bytes: u32,
}

/// Owned root effects held after the decision they follow (domain/engine.md, section 5.2). External notices have no acknowledgement of their own; web
/// replies consume one reply destination. Internal fleet/load deliveries are
/// resumed by the root and never passed to the protocol as child events.
#[derive(PartialEq, Eq, Debug)]
pub enum Delivery {
    /// Typed protocol handoffs follow their prerequisite commits.
    Host(Box<crate::engine::HostDelivery>),
    /// Release a committed connector entry to its client.
    ForgeCommitted { entry: u64 },
    /// One bounded connector call after all preceding progress commits.
    ForgeCall {
        call: Token,
        repository: temper_engine_domain_forge_client::api::Repository,
        op: temper_engine_domain_forge_client::api::Op,
    },
    /// Internal committed notice to the expendable live views child.
    View(Box<jig_core_views::Event>),
    /// Committed request to the connector that owns a procedure task. The owner steps against
    /// current facts and returns the fenced decision through the root.
    Procedure { task: u64, step: u64, connector: u16, code: u32 },
    /// One fleet-fenced named tool answer after its decision is durable.
    CallAnswer { channel: Token, task: u64, attempt: u64, call: Token, answer: crate::CallAnswer },
    /// Committed words presented to fleet only after their task row commits.
    Relay { task: u64, attempt: u64, previous: Option<u64>, word: jig_core_tasks::Word },
    /// Fleet-selected live worker receives the whole committed word.
    Inbound { channel: Token, task: u64, attempt: u64, word: jig_core_tasks::Word },
    /// Bounded page of committed unread results, consumed through its last position with this reply.
    InboxPage { to: ReplyTo, person: u64, entries: Box<[ResultEntry]> },
    /// One authenticated newest-first page derived from committed task rows.
    InboxView { to: ReplyTo, person: u64, entries: Box<[InboxViewEntry]>, next: Option<InboxCursor> },
    /// Root-internal start of the store-backed whole inbox scan after prior commits.
    BeginInboxView { waiter: Token },
    /// Root-to-authenticated named reader: one current durable held-chat view,
    /// derived from tasks without a persistent people inbox.
    EscalationReply {
        /// One web-issued reply right.
        to: ReplyTo,
        person: u64,
        /// Bounded semantic view; rejection owns at most `result_bytes`
        context: Box<jig_core_tasks::EscalationContext>,
    },
    /// Root internal historical read, released after preceding commit; consumed
    /// once by root and never exposed to the shell.
    ReadEscalationDecision {
        /// Reserved root read slot, retired after IO terminal.
        waiter: Token,
    },
    /// Journal caller to web: one people terminal without an attached session; the walking root
    /// uses `WebReply` instead.
    Reply {
        /// Web-issued right to exactly this terminal; moved once.
        to: ReplyTo,
        /// Typed, secret-free reply with no owned unbounded body.
        reply: people::Reply,
    },
    /// Root to worker: accepted turn is durable; release its retained body. This notice has no
    /// terminal event.
    AcknowledgeTurn {
        /// Worker protocol destination echoed through fleet.
        channel: Token,
        /// Durable task whose turn was accepted.
        task: u64,
        /// Root-issued activation fence for that turn.
        attempt: u64,
        /// Positive accepted turn number, represented by `u32`.
        turn: u32,
    },
    /// Root to worker: the attempt terminal is durable; release its answer. This notice has no
    /// terminal event.
    Acknowledge {
        /// Worker protocol destination echoed through fleet.
        channel: Token,
        /// Durable task whose attempt answered.
        task: u64,
        attempt: u64,
    },
    /// Root to worker: stop this attempt. Its answer or lost-channel grace outcome returns through
    /// fleet; this notice allocates no additional attempt.
    Cancel {
        /// Worker protocol destination echoed through fleet.
        channel: Token,
        task: u64,
        /// Root-issued activation to cancel, not a later attempt.
        attempt: u64,
    },
    /// Root to fleet after durability; the root resumes it internally.
    Fleet(
        /// Root-issued `Start`, `TurnKept`, `Acknowledge` or `Cancel` callback. `Start` workstream
        /// bytes are at most journal `transcript_bytes`; other payloads are fixed-size. Fleet
        /// start/cancel consequences route back inside the root.
        jig_core_fleet::Event,
    ),
    /// Root to worker: complete bounded assignment after its claim commit.
    Assigned {
        /// Worker protocol destination, bounded by fleet worker room.
        channel: Token,
        /// Complete claim/brief/grant; section slots and owned bytes validated at journal
        /// admission.
        assignment: crate::engine::Assignment,
    },
    /// Root to web: one terminal reply, optionally carrying its sign-in candidate; only a
    /// successful people reply admits that session.
    WebReply {
        to: ReplyTo,
        /// Root-issued sign-in candidate accompanying a sign-in decision, or none. A refusal can
        /// leave it unused; clients require a successful `SignedIn` reply before using it.
        sign_in: Option<u64>,
        /// Fixed-size secret-free people terminal, held after the decision it names; owns no
        /// display or credential bytes.
        reply: people::Reply,
    },
    /// Root to worker: refuse its hello because of startup, shape or fleet admission; no attempt is
    /// assigned and no terminal is owed for this notice.
    Refuse {
        /// Fixed-size worker protocol destination echoed from its refused hello; no worker slot is
        /// admitted.
        channel: Token,
    },
    /// Root resumes authenticated historical result IO only after prior commits.
    ReadResult {
        /// Root-owned load waiter identity; internal routing consumes this notice once.
        waiter: Token,
    },
    /// Root to worker: fleet lacked turn admission room; keep the body and retry after a backoff,
    /// without charging.
    TurnBusy {
        /// Current worker protocol channel.
        channel: Token,
        /// Durable task/claim identity.
        task: u64,
        /// Activation fence checked by fleet.
        attempt: u64,
        /// Positive turn retained by the sender for retry.
        turn: u32,
    },
    /// Root to store after its prerequisite commits, resumed internally.
    Load {
        /// Root-owned load waiter identity; internal routing consumes this notice once.
        waiter: Token,
        /// Closed key range, validated by the root load child.
        range: crate::Range,
        /// Exclusive validated page continuation, or range beginning.
        after: Option<Key>,
    },
    /// Root to authenticated web reader: terminal derived from an ended task.
    ResultReply {
        to: ReplyTo,
        /// Authenticated requester number from people session and ended task.
        person: u64,
        /// Durable ended task number naming this result, never a separate inbox record.
        task: u64,
        /// Result position committed as read with this reply.
        position: u64,
        /// Owned result text bounded by journal `result_bytes`.
        words: Box<[u8]>,
    },
    /// Root to the task requester: live notice of its durable ending, with no reply right.
    /// The unread entry remains derived from the ended task until its read position advances.
    Result {
        /// Durable person requester, checked by the task route.
        person: u64,
        task: u64,
        /// Owned result text, at most journal `result_bytes`.
        words: Box<[u8]>,
    },
}

/// One result derived from a stored ended task; no separate person message row exists.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ResultEntry {
    pub position: u64,
    pub task: u64,
    /// Checked report or failure words from the ended task.
    pub words: Box<[u8]>,
}

/// Stable descending page cursor for a task-derived person inbox.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InboxCursor {
    pub at: Wall,
    pub task: u64,
    pub kind: u8,
    pub number: u64,
    /// Highest result or reply position seen on the first page, carried through the scan.
    pub read_high: u64,
}

/// One visible task-derived item; historical result words remain bounded by the root.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InboxViewEntry {
    /// A current question, decision, person task or reply.
    Waiting(people::Entry),
    /// A historical result requested by this person and still unread.
    Result { at: Wall, result: ResultEntry },
}

impl InboxViewEntry {
    /// Cursor used to page this item's committed ordering without retaining history in the root.
    #[must_use]
    pub const fn cursor(&self) -> InboxCursor {
        match self {
            InboxViewEntry::Waiting(entry) => {
                let (kind, number) = match entry.kind {
                    people::EntryKind::Question { message } => (1, message),
                    people::EntryKind::Proposal { number } => (2, number),
                    people::EntryKind::Escalation { revision } => (3, revision),
                    people::EntryKind::PersonTask => (4, 0),
                    people::EntryKind::Reply { message } => (5, message),
                };
                InboxCursor { at: entry.at, task: entry.task, kind, number, read_high: 0 }
            }
            InboxViewEntry::Result { at, result } => {
                InboxCursor { at: *at, task: result.task, kind: 6, number: result.position, read_high: 0 }
            }
        }
    }
}

/// Journal to its root caller: at most one value per entry point; the caller
/// translates it to store IO or an outward/internal delivery (domain/engine.md, 5).
#[derive(PartialEq, Eq, Debug)]
pub enum Output {
    /// Output that decides nothing, admitted through the journal's door.
    Now(crate::engine::Request),
    /// Ordered atomic store transaction, ended by `committed` or `uncommitted` using the issued
    /// number.
    Commit {
        /// Positive monotonically allocated commit identity.
        number: u64,
        /// Owned unique-key writes including the header, at most `Limits::writes`.
        writes: Box<[Write]>,
    },
    /// One effect whose prerequisite commit is durable.
    Deliver(/** Owned bounded effect, consumed once by the root. */ Delivery),
    /// Storage failed; the caller stops the process without releasing later work. This notice has
    /// no terminal event.
    Stop,
}

/// One root route's bounded transient writes and deliveries; it owns their
/// payloads until accepted or returned on refusal (domain/engine.md, 5.1–5.2).
/// This value knows no durability and emits nothing on drop.
#[derive(Debug)]
pub struct Decision {
    limits: Limits,
    reserved: JournalRoom,
    writes: List<Write>,
    deliveries: Queue<Delivery>,
    overrun: bool,
    batch: Option<SkeinDecision<Write, Output>>,
}

/// skein-lib's bounded commit barrier, over the root's wrapped writes and outputs.
pub type Journal = SkeinJournal<Write, Output>;

impl Decision {
    /// Test the unused slots while exercising journal admission bounds.
    #[cfg(test)]
    pub(crate) fn room_for(&self, writes: u32, deliveries: u32) -> bool {
        self.reserved.writes.saturating_sub(self.writes.len().saturating_add(1)) >= writes
            && self.reserved.held.saturating_sub(self.deliveries.len()) >= deliveries
    }

    /// Allocate one transient decision from validated startup limits; no effect is issued until
    /// `accept`.
    #[must_use]
    pub fn new(limits: &Limits) -> Decision {
        assert!(worst_case(limits).is_some(), "valid root journal limits");
        Decision {
            limits: *limits,
            reserved: room(limits),
            writes: List::with_capacity(limits.writes.checked_sub(1).expect("header slot reserved")),
            deliveries: Queue::with_capacity(limits.deliveries),
            overrun: false,
            batch: None,
        }
    }

    /// Reserve the generic journal's worst-case room before routing a child.
    pub fn reserve(journal: &mut Journal, limits: &Limits) -> Option<Decision> {
        Self::reserve_room(journal, limits, room(limits))
    }

    /// Reserve a route's checked write and held-output counts before any child changes.
    pub(crate) fn reserve_room(journal: &mut Journal, limits: &Limits, reserved: JournalRoom) -> Option<Decision> {
        if reserved.writes == 0 || reserved.writes > limits.writes || reserved.held > limits.deliveries {
            return None;
        }
        let batch = journal.decision(&reserved)?;
        let mut decision = Decision::new(limits);
        decision.reserved = reserved;
        decision.batch = Some(batch);
        Some(decision)
    }

    /// A replacement at the same key keeps its original position. All child callbacks finish before
    /// submission, so only the final value is saved. The root supplies the write; successful
    /// admission emits no event. Bounds or capacity refusal returns it unchanged. Deployment writes
    /// are reserved for `accept`, not callers.
    #[expect(clippy::result_large_err, reason = "journal refusal returns the caller's owned bounded row")]
    pub fn write(&mut self, limits: &Limits, write: Write) -> Result<(), Write> {
        assert!(*limits == self.limits, "decision uses its configured limits");
        let within = match &write {
            Write::Save(Record::Deployment(_))
            | Write::Erase(Key::Deployment | Key::EscalationDecision { .. } | Key::ProposalDecision(_)) => false,
            Write::Save(Record::Turn(row)) => {
                row.task != 0
                    && row.attempt != 0
                    && row.turn != 0
                    && row.transcript.len() <= usize::try_from(limits.transcript_bytes).expect("u32 fits usize")
            }
            Write::Save(Record::Call(row)) => row.key.task != 0 && row.key.attempt != 0 && row.key.completion != 0,
            Write::Erase(
                Key::Call(_)
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_)
                | Key::Forge(_)
                | Key::Projection(_),
            ) => true,
            Write::Save(Record::EscalationDecision(row)) => {
                row.task != 0
                    && row.revision != 0
                    && row.by != 0
                    && row.requester != 0
                    && match &row.decision {
                        jig_core_people::EscalationDecision::Release | jig_core_people::EscalationDecision::Pass => {
                            true
                        }
                        jig_core_people::EscalationDecision::Reject { reason } => {
                            reason.len()
                                <= usize::try_from(limits.result_bytes.min(limits.transcript_bytes))
                                    .expect("u32 fits usize")
                        }
                    }
            }
            Write::Save(Record::ProposalDecision(row)) => row.project != 0 && row.proposal != 0 && row.by != 0,
            Write::Erase(Key::Notes(_))
            | Write::Save(
                Record::Tasks(_)
                | Record::Projection(_)
                | Record::People(_)
                | Record::Notes(_)
                | Record::Forge { .. }
                | Record::RunProof(_)
                | Record::Terminal(_),
            ) => match crate::store::owned_bytes(&write) {
                Some(bytes) => bytes <= u64::from(limits.transcript_bytes),
                None => false,
            },
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
        if self.writes.len() >= self.reserved.writes.checked_sub(1).expect("header slot reserved") {
            self.overrun = true;
            return Err(write);
        }
        let result = self.writes.push(write);
        if result.is_err() {
            self.overrun = true;
        }
        result
    }

    /// Retain one root effect for the decision, without releasing or issuing it. Bounds or capacity
    /// refusal returns ownership; successful admission ends through journal release after
    /// durability.
    #[expect(clippy::result_large_err, reason = "bounded assignment ownership is returned intact on admission refusal")]
    #[expect(clippy::too_many_lines, reason = "one exhaustive delivery bound check")]
    pub fn deliver(&mut self, limits: &Limits, delivery: Delivery) -> Result<(), Delivery> {
        assert!(*limits == self.limits, "decision uses its configured limits");
        let within = match &delivery {
            Delivery::View(event) => match event.as_ref() {
                jig_core_views::Event::TaskPhase { trees, .. } => {
                    trees.len() <= usize::try_from(limits.deliveries).expect("u32 fits usize")
                }
                jig_core_views::Event::Started { .. }
                | jig_core_views::Event::Turn { .. }
                | jig_core_views::Event::Finished { .. }
                | jig_core_views::Event::Inbox { .. } => true,
                jig_core_views::Event::Reported { .. }
                | jig_core_views::Event::Watch { .. }
                | jig_core_views::Event::Unwatch { .. }
                | jig_core_views::Event::Delivered { .. } => false,
            },
            Delivery::Result { words, .. } | Delivery::ResultReply { words, .. } => {
                words.len() <= usize::try_from(limits.result_bytes).expect("u32 fits usize")
            }
            Delivery::InboxPage { entries, .. } => {
                let mut bytes = Some(0_u64);
                for entry in entries {
                    if entry.words.len() > usize::try_from(limits.result_bytes).expect("u32 fits usize") {
                        bytes = None;
                        break;
                    }
                    bytes = match bytes {
                        Some(total) => match total
                            .checked_add(u64::try_from(size_of::<ResultEntry>()).expect("usize fits u64"))
                        {
                            Some(total) => total.checked_add(u64::try_from(entry.words.len()).expect("usize fits u64")),
                            None => None,
                        },
                        None => None,
                    };
                }
                match bytes {
                    Some(bytes) => bytes <= u64::from(limits.transcript_bytes),
                    None => false,
                }
            }
            Delivery::InboxView { entries, .. } => {
                let mut bytes = 0_u64;
                let mut valid = true;
                for entry in entries {
                    let extra = match entry {
                        InboxViewEntry::Waiting(_) => 0,
                        InboxViewEntry::Result { result, .. } => {
                            if result.words.len() > usize::try_from(limits.result_bytes).expect("u32 fits usize") {
                                valid = false;
                            }
                            u64::try_from(result.words.len()).expect("usize fits u64")
                        }
                    };
                    let Some(total) =
                        bytes.checked_add(u64::try_from(size_of::<InboxViewEntry>()).expect("usize fits u64"))
                    else {
                        valid = false;
                        break;
                    };
                    let Some(next) = total.checked_add(extra) else {
                        valid = false;
                        break;
                    };
                    bytes = next;
                }
                valid && bytes <= u64::from(limits.transcript_bytes)
            }
            Delivery::EscalationReply { context, .. } => {
                context.task != 0
                    && match &context.escalation {
                        jig_core_tasks::Escalation::Rejected { revision, by, reason } => {
                            *revision != 0
                                && *by != 0
                                && reason.len() <= usize::try_from(limits.result_bytes).expect("u32 fits usize")
                        }
                        jig_core_tasks::Escalation::Waiting { revision, holder, .. } => {
                            *revision != 0
                                && match holder {
                                    jig_core_tasks::EscalationHolder::Task(task) => *task != 0,
                                    jig_core_tasks::EscalationHolder::Person(person) => {
                                        *person != 0 && *person == context.requester
                                    }
                                    jig_core_tasks::EscalationHolder::Role { project, .. } => {
                                        *project == context.project
                                    }
                                }
                        }
                        jig_core_tasks::Escalation::Unheld { .. } | jig_core_tasks::Escalation::Routing { .. } => false,
                    }
            }
            Delivery::Fleet(event) => fleet_delivery_within(event),
            Delivery::Relay { word, .. } | Delivery::Inbound { word, .. } => {
                word.number != 0
                    && word.words.len() <= usize::try_from(limits.transcript_bytes).expect("u32 fits usize")
            }
            Delivery::Assigned { assignment, .. } => assignment_within(assignment, limits),
            Delivery::Host(host) => host_within(host, limits),
            Delivery::CallAnswer { .. }
            | Delivery::ForgeCommitted { .. }
            | Delivery::ForgeCall { .. }
            | Delivery::Procedure { .. }
            | Delivery::Reply { .. }
            | Delivery::AcknowledgeTurn { .. }
            | Delivery::Acknowledge { .. }
            | Delivery::Cancel { .. }
            | Delivery::WebReply { .. }
            | Delivery::Refuse { .. }
            | Delivery::ReadEscalationDecision { .. }
            | Delivery::ReadResult { .. }
            | Delivery::BeginInboxView { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Load { .. } => true,
        };
        if !within {
            return Err(delivery);
        }
        if self.deliveries.len() >= self.reserved.held {
            self.overrun = true;
            return Err(delivery);
        }
        let result = self.deliveries.try_push(delivery);
        if result.is_err() {
            self.overrun = true;
        }
        result
    }
}

/// Translate temper's limits to skein-lib's generic journal capacities.
#[must_use]
pub const fn journal_limits(limits: &Limits) -> JournalLimits {
    JournalLimits {
        commits: limits.commits,
        writes: limits.writes,
        held: limits.held,
        now: limits.deliveries,
        release: 1,
    }
}

/// Reserve the worst-case write and held room for one root route.
#[must_use]
pub const fn room(limits: &Limits) -> JournalRoom {
    JournalRoom { writes: limits.writes, held: limits.deliveries }
}

/// Called before routing anything that may mutate a child. Reserving the whole decision avoids
/// partially applying a child and discovering pressure. Pure query for one journal batch, with no
/// mutation or emitted terminal; the walking root additionally reserves callback room.
#[must_use]
pub fn takes(journal: &Journal, limits: &Limits) -> bool {
    journal.takes(&room(limits))
}

/// Return ownership on refusal. The root must call `takes` before making the decision; this
/// defensive check does not undo already-routed children. A dirty header or
/// nonempty write set produces one atomic commit, ended by the store; a read-only decision issues
/// none. Every retained delivery follows the latest allocated commit.
#[expect(clippy::result_large_err, reason = "refused root decisions return all owned writes and outputs")]
pub fn accept_pending(
    journal: &mut Journal,
    counters: &mut Counters,
    limits: &Limits,
    mut decision: Decision,
) -> Result<(), Decision> {
    if decision.overrun {
        let mut batch = match decision.batch.take() {
            Some(batch) => batch,
            None => journal
                .decision(&JournalRoom { writes: 0, held: 0 })
                .expect("overrun follows an admitted root decision"),
        };
        for _ in 0..=limits.writes {
            if batch.write(Write::Erase(Key::Deployment)).is_err() {
                break;
            }
        }
        journal.accept(batch);
        return Ok(());
    }
    if (decision.batch.is_none() && !takes(journal, limits))
        || decision.limits != *limits
        || decision.writes.len() >= limits.writes
        || decision.deliveries.len() > limits.deliveries
    {
        return Err(decision);
    }
    let writing = counters.dirty() || !decision.writes.is_empty();
    let mut batch = match decision.batch.take() {
        Some(batch) => batch,
        None => journal.decision(&room(limits)).expect("preflight reserved the whole decision"),
    };
    if writing {
        batch.write(Write::Save(Record::Deployment(counters.next_commit()))).expect("header slot reserved");
        for at in 0..decision.writes.len() {
            let source = decision.writes.get_mut(at).expect("admitted write index");
            let key = source.key();
            // The source list is consumed in this step. An erase is a
            // payload-free terminal placeholder, never submitted again.
            let write = core::mem::replace(source, Write::Erase(key));
            batch.write(write).expect("admitted decision writes");
        }
    }
    for _ in 0..decision.deliveries.len() {
        let delivery = decision.deliveries.pop().expect("admitted delivery count");
        batch.hold(Output::Deliver(delivery)).expect("admitted held output");
    }
    journal.accept(batch);
    Ok(())
}

/// Take the next numbered journal commit for the store.
pub fn commit(journal: &mut Journal, limits: &Limits) -> Option<Output> {
    let commit = journal.commit()?;
    let mut writes = List::with_capacity(limits.writes);
    let mut rows = commit.writes;
    while let Some(write) = rows.pop() {
        writes.push(write).expect("commit writes bounded by journal limits");
    }
    Some(Output::Commit { number: commit.number, writes: writes.into_boxed() })
}

/// Compatibility seam for the journal's focused world: accept and take its commit.
#[expect(clippy::result_large_err, reason = "refused root decisions return all owned writes and outputs")]
pub fn accept(
    journal: &mut Journal,
    counters: &mut Counters,
    limits: &Limits,
    decision: Decision,
    out: &mut Queue<Output>,
) -> Result<(), Decision> {
    assert!(out.room() >= 1, "one root journal output reserved");
    accept_pending(journal, counters, limits, decision)?;
    if let Some(output) = commit(journal, limits) {
        out.push(output);
    }
    Ok(())
}

/// A cumulative store answer only makes outputs ready. The ready pass releases one per call; it
/// never drains all of them inside the store step. The store echoes an issued `u64` commit number.
/// Duplicate/older terminals are inert; a number beyond issued commits is a caller error.
pub fn committed(journal: &mut Journal, number: u64) {
    journal.committed(number);
}

/// Root ready pass: reserve one output and release at most one durable held effect in FIFO order;
/// stopped or unready journals emit nothing. A delivery is consumed by its root route, not answered
/// to this seam.
pub fn resume(journal: &mut Journal, out: &mut Queue<Output>) {
    assert!(out.room() >= 1, "one root ready output reserved");
    let _released: skein_lib::Released = journal.release(out);
}

/// Store to journal: an issued commit failed. Reserve one output; the first failure beyond durable
/// progress emits `Stop` and retains all held effects. Older or repeated failure notices are inert.
pub fn uncommitted(journal: &mut Journal, number: u64, out: &mut Queue<Output>) {
    assert!(out.room() >= 1, "one root stop output reserved");
    if journal.stopped() {
        return;
    }
    journal.failed(number);
    out.push(Output::Stop);
}

/// Pure startup heap calculation for journal containers and simultaneously owned
/// decision/commit/held payloads, excluding allocator overhead. Invalid bounds or arithmetic
/// overflow return `None`; no state or effect is created.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.commits == 0
        || limits.writes == 0
        || limits.held < limits.deliveries
        || limits.deliveries == 0
        || limits.run_bytes == 0
    {
        return None;
    }
    Journal::worst_case(&journal_limits(limits))?
        .checked_add(List::<Write>::worst_case(limits.writes)?.checked_mul(2)?)?
        .checked_add(Queue::<Delivery>::worst_case(limits.deliveries)?)?
        .checked_add(u64::from(limits.writes).checked_mul(2)?.checked_mul(u64::from(limits.transcript_bytes))?)?
        .checked_add(
            u64::from(limits.held).checked_add(u64::from(limits.deliveries))?.checked_mul(
                u64::from(limits.result_bytes)
                    .max(u64::from(limits.transcript_bytes))
                    .checked_add(u64::from(limits.run_bytes))?
                    .checked_add(
                        List::<crate::engine::BriefSection>::worst_case(limits.deliveries)?
                            .max(u64::try_from(size_of::<jig_core_tasks::EscalationContext>()).ok()?),
                    )?,
            )?,
        )
}

fn fleet_delivery_within(event: &jig_core_fleet::Event) -> bool {
    use jig_core_fleet::Event;
    match event {
        Event::Start { .. }
        | Event::TurnKept { .. }
        | Event::Acknowledge { .. }
        | Event::Cancel { .. }
        | Event::Relayed { .. } => true,
        Event::Inbound { .. }
        | Event::Relay { .. }
        | Event::Adopt { .. }
        | Event::Loaded
        | Event::Grant { .. }
        | Event::Rejected { .. }
        | Event::Exhausted { .. }
        | Event::Hello { .. }
        | Event::Lost { .. }
        | Event::Answer { .. }
        | Event::Turn { .. }
        | Event::TurnBusy { .. }
        | Event::Bounced { .. }
        | Event::Told { .. } => false,
    }
}

fn assignment_within(assignment: &crate::engine::Assignment, limits: &Limits) -> bool {
    let charter_within = match crate::engine::run_charter_bytes(&assignment.run) {
        Some(bytes) => bytes <= u64::from(limits.run_bytes),
        None => false,
    };
    if !charter_within {
        return false;
    }
    if assignment.task == 0
        || assignment.attempt == 0
        || assignment.sections.len() > usize::try_from(limits.deliveries).expect("u32 fits usize")
        || assignment.saved.len() > usize::try_from(limits.deliveries).expect("u32 fits usize")
    {
        return false;
    }
    let mut previous_saved = 0;
    for tag in &assignment.saved {
        if *tag <= previous_saved {
            return false;
        }
        previous_saved = *tag;
    }
    let mut owned = 0_u64;
    let mut last = 0_u64;
    for word in &assignment.inbox {
        if word.number == 0 || word.number <= last || word.words.is_empty() {
            return false;
        }
        last = word.number;
        let Some(total) = owned.checked_add(u64::try_from(word.words.len()).expect("usize fits u64")) else {
            return false;
        };
        owned = total;
    }
    if owned > u64::from(limits.transcript_bytes) {
        return false;
    }
    let inbox_bytes = owned;
    owned = 0;
    for section in &assignment.sections {
        let bytes = match &section.body {
            crate::engine::BriefBody::Text(bytes) => u64::try_from(bytes.len()).expect("usize fits u64"),
            crate::engine::BriefBody::Missing(_) => 0,
        };
        let Some(total) = owned.checked_add(bytes) else {
            return false;
        };
        owned = total;
    }
    let mut transcript = 0_u64;
    for turn in &assignment.transcript {
        let Some(total) = transcript.checked_add(u64::try_from(turn.len()).expect("usize fits u64")) else {
            return false;
        };
        transcript = total;
    }
    if !answered_within(assignment, limits) {
        return false;
    }
    let answered_bytes = u64::try_from(assignment.answered.len())
        .expect("usize fits u64")
        .checked_mul(u64::try_from(size_of::<crate::CallRecord>()).expect("usize fits u64"));
    let total = match (transcript.checked_add(inbox_bytes), answered_bytes) {
        (Some(transcript), Some(answered)) => transcript.checked_add(answered),
        (None, _) | (_, None) => None,
    };
    let within = match total {
        Some(all) => all <= u64::from(limits.transcript_bytes),
        None => false,
    };
    owned <= u64::from(limits.result_bytes) && within
}

fn host_within(host: &crate::engine::HostDelivery, limits: &Limits) -> bool {
    match host {
        crate::engine::HostDelivery::Render { name, tool, answer, .. } => {
            length_within(name.len().checked_add(tool.len()), limits.transcript_bytes)
                && bytes_within(crate::store::call_answer_bytes(answer), limits.transcript_bytes)
        }
        crate::engine::HostDelivery::Answer { call, .. } => match call.owned_bytes() {
            Some(bytes) => bytes <= u64::from(limits.transcript_bytes).checked_add(4).expect("bounded busy answer"),
            None => false,
        },
        crate::engine::HostDelivery::Inbound { message, .. } => {
            length_within(message.sender.len().checked_add(message.words.len()), limits.transcript_bytes)
        }
    }
}

pub(crate) fn length_within(bytes: Option<usize>, limit: u32) -> bool {
    match bytes {
        Some(bytes) => bytes <= usize::try_from(limit).expect("u32 fits usize"),
        None => false,
    }
}

fn bytes_within(bytes: Option<u64>, limit: u32) -> bool {
    match bytes {
        Some(bytes) => bytes <= u64::from(limit),
        None => false,
    }
}

fn answered_within(assignment: &crate::engine::Assignment, limits: &Limits) -> bool {
    let mut previous = None;
    for row in &assignment.answered {
        let key = row.key;
        if key.task != assignment.task
            || key.attempt == 0
            || key.attempt >= assignment.attempt
            || key.completion == 0
            || match previous {
                Some(old) => key <= old,
                None => false,
            }
        {
            return false;
        }
        previous = Some(key);
    }
    let mut serial = 0;
    for call in &assignment.settled {
        if call.serial <= serial
            || call.name.is_empty()
            || call.tool.is_empty()
            || !bytes_within(call.owned_bytes(), limits.transcript_bytes)
        {
            return false;
        }
        serial = call.serial;
    }
    for row in &assignment.answered {
        if !bytes_within(crate::store::call_answer_bytes(&row.answer), limits.transcript_bytes)
            || match &row.settled {
                Some(call) => !bytes_within(call.owned_bytes(), limits.transcript_bytes),
                None => false,
            }
        {
            return false;
        }
    }
    true
}
