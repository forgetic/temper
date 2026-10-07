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
use core::mem::size_of;
use skein_lib::{List, Queue, ReplyTo, Token, Wall};
use temper_engine_domain_people as people;

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
    /// Release a committed connector entry to its client.
    ForgeCommitted { entry: u64 },
    /// One bounded connector call after all preceding progress commits.
    ForgeCall {
        call: Token,
        repository: temper_engine_domain_forge_client::api::Repository,
        op: temper_engine_domain_forge_client::api::Op,
    },
    /// Internal committed notice to the expendable live views child.
    View(Box<temper_engine_domain_views::Event>),
    /// Committed request to the connector that owns a procedure task. The owner steps against
    /// current facts and returns the fenced decision through the root.
    Procedure { task: u64, step: u64, connector: u16, code: u32 },
    /// One fleet-fenced named tool answer after its decision is durable.
    CallAnswer { channel: Token, task: u64, attempt: u64, call: Token, answer: crate::CallAnswer },
    /// Committed words presented to fleet only after their task row commits.
    Relay { task: u64, attempt: u64, previous: Option<u64>, word: temper_engine_domain_tasks::Word },
    /// Fleet-selected live worker receives the whole committed word.
    Inbound { channel: Token, task: u64, attempt: u64, word: temper_engine_domain_tasks::Word },
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
        context: Box<temper_engine_domain_tasks::EscalationContext>,
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
        temper_engine_domain_fleet::Event,
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

    /// Allocate one transient decision from validated startup limits; no effect is issued until
    /// `accept`.
    #[must_use]
    pub fn new(limits: &Limits) -> Decision {
        assert!(worst_case(limits).is_some(), "valid root journal limits");
        Decision {
            limits: *limits,
            writes: List::with_capacity(limits.writes.checked_sub(1).expect("header slot reserved")),
            deliveries: Queue::with_capacity(limits.deliveries),
        }
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
                | Key::Forge(_),
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
            Write::Save(Record::ProposalDecision(row)) => row.project != 0 && row.proposal != 0 && row.by != 0,
            Write::Save(
                Record::Tasks(_) | Record::People(_) | Record::Forge { .. } | Record::RunProof(_) | Record::Terminal(_),
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
        self.writes.push(write)
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
                temper_engine_domain_views::Event::TaskPhase { trees, .. } => {
                    trees.len() <= usize::try_from(limits.deliveries).expect("u32 fits usize")
                }
                temper_engine_domain_views::Event::Started { .. }
                | temper_engine_domain_views::Event::Turn { .. }
                | temper_engine_domain_views::Event::Finished { .. }
                | temper_engine_domain_views::Event::Inbox { .. } => true,
                temper_engine_domain_views::Event::Reported { .. }
                | temper_engine_domain_views::Event::Watch { .. }
                | temper_engine_domain_views::Event::Unwatch { .. }
                | temper_engine_domain_views::Event::Delivered { .. } => false,
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
                        temper_engine_domain_tasks::Escalation::Rejected { revision, by, reason } => {
                            *revision != 0
                                && *by != 0
                                && reason.len() <= usize::try_from(limits.result_bytes).expect("u32 fits usize")
                        }
                        temper_engine_domain_tasks::Escalation::Waiting { revision, holder, .. } => {
                            *revision != 0
                                && match holder {
                                    temper_engine_domain_tasks::EscalationHolder::Task(task) => *task != 0,
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
            Delivery::Relay { word, .. } | Delivery::Inbound { word, .. } => {
                word.number != 0
                    && word.words.len() <= usize::try_from(limits.transcript_bytes).expect("u32 fits usize")
            }
            Delivery::Assigned { assignment, .. } => assignment_within(assignment, limits),
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
        self.deliveries.try_push(delivery)
    }
}

impl Journal {
    /// An empty store needs the deployment identity committed even before its first task. The id is
    /// a root input, drawn once by the shell at startup. Validated limits size the fixed held
    /// queue; the next accepted decision emits the dirty header's commit.
    #[must_use]
    pub fn bootstrap(id: [u8; 16], limits: &Limits) -> Journal {
        let deployment = Deployment {
            id,
            tasks: 0,
            people: 0,
            sign_ins: 0,
            messages: 0,
            runs: 0,
            calls: 0,
            forge_rows: 0,
            commits: 0,
        };
        let mut journal = Journal::new(deployment, limits);
        journal.dirty = true;
        journal
    }

    /// A loaded header names the last fully applied commit. It is durable; outstanding answers from
    /// a previous process need not be reconstructed. The root supplies the decoded store header and
    /// validated startup bounds; construction emits no effect.
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

    /// Pure snapshot of allocated counters, which may run ahead of the durable store until its
    /// issued commits answer.
    #[must_use]
    pub const fn deployment(&self) -> Deployment {
        self.deployment
    }

    /// Pure query for the last cumulatively answered commit; bounded by the allocated commit
    /// counter.
    #[must_use]
    pub const fn durable(&self) -> u64 {
        self.durable
    }

    /// Pure query for the storage-failure fence; true prevents subsequent admission and release.
    #[must_use]
    pub const fn stopped(&self) -> bool {
        self.stopped
    }

    /// Pure shell idle query: no dirty header, held delivery or unanswered issued commit remains. A
    /// stopped journal is not complete.
    #[must_use]
    pub fn quiescent(&self) -> bool {
        !self.stopped && !self.dirty && self.held.is_empty() && self.durable == self.deployment.commits
    }

    /// Pure root query for unused held-delivery slots; callback admission uses this in addition to
    /// `takes`.
    pub(crate) fn held_room(&self) -> u32 {
        self.held.room()
    }

    /// Pure query: whether the front held effect can be released by `resume`. It reports no child
    /// or IO readiness.
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

/// Called before routing anything that may mutate a child. Reserving the whole decision avoids
/// partially applying a child and discovering pressure. Pure query for one journal batch, with no
/// mutation or emitted terminal; the walking root additionally reserves callback room.
#[must_use]
pub fn takes(journal: &Journal, limits: &Limits) -> bool {
    assert!(*limits == journal.limits, "journal uses its configured limits");
    !journal.stopped
        && journal.deployment.commits != u64::MAX
        && journal.deployment.commits.checked_sub(journal.durable).expect("durable never exceeds made")
            < u64::from(limits.commits)
        && journal.held.room() >= limits.deliveries
}

/// Root numbers are never reused. Allocation is part of the admitted decision, even if its
/// candidate is unused; the next commit saves the gap. The root chooses the counter family after
/// admission. Returns its next positive `u64`, or `None` when stopped/exhausted, with no effect on
/// refusal; emits no request itself.
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
        Family::ForgeRow => &mut journal.deployment.forge_rows,
    };
    let next = counter.checked_add(1)?;
    *counter = next;
    journal.dirty = true;
    Some(next)
}

/// Return ownership on refusal. The root must call `takes` before making the decision; this
/// defensive check does not undo already-routed children. Reserve one output. A dirty header or
/// nonempty write set produces one atomic commit, ended by the store; a read-only decision issues
/// none. Every retained delivery follows the latest allocated commit.
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

/// A cumulative store answer only makes outputs ready. The ready pass releases one per call; it
/// never drains all of them inside the store step. The store echoes an issued `u64` commit number.
/// Duplicate/older terminals are inert; a number beyond issued commits is a caller error.
pub fn committed(journal: &mut Journal, number: u64) {
    if journal.stopped || number <= journal.durable {
        return;
    }
    assert!(number <= journal.deployment.commits, "store answers only issued commits");
    journal.durable = number;
}

/// Root ready pass: reserve one output and release at most one durable held effect in FIFO order;
/// stopped or unready journals emit nothing. A delivery is consumed by its root route, not answered
/// to this seam.
pub fn resume(journal: &mut Journal, out: &mut Queue<Output>) {
    assert!(out.room() >= 1, "one root ready output reserved");
    if journal.ready() {
        let held = journal.held.pop().expect("ready front exists");
        out.push(Output::Deliver(held.delivery));
    }
}

/// Store to journal: an issued commit failed. Reserve one output; the first failure beyond durable
/// progress emits `Stop` and retains all held effects. Older or repeated failure notices are inert.
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
    Queue::<Held>::worst_case(limits.held)?
        .checked_add(List::<Write>::worst_case(limits.writes)?.checked_mul(2)?)?
        .checked_add(Queue::<Delivery>::worst_case(limits.deliveries)?)?
        .checked_add(u64::from(limits.writes).checked_mul(2)?.checked_mul(u64::from(limits.transcript_bytes))?)?
        .checked_add(
            u64::from(limits.held).checked_add(u64::from(limits.deliveries))?.checked_mul(
                u64::from(limits.result_bytes)
                    .max(u64::from(limits.transcript_bytes))
                    .checked_add(u64::from(limits.run_bytes))?
                    .checked_add(
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
        Event::TurnKept { .. } | Event::Acknowledge { .. } | Event::Cancel { .. } | Event::Relayed { .. } => true,
        Event::Adopt { .. }
        | Event::Inbound { .. }
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
            temper_engine_domain_brief::Body::Text(bytes) => u64::try_from(bytes.len()).expect("usize fits u64"),
            temper_engine_domain_brief::Body::Missing(_) => 0,
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
