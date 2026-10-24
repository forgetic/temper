//! The root's ordered commit seam (domain/root.md, section 4).
//! [`Decision`] keeps a bounded unique-key write set and owned deliveries;
//! skein-lib's [`Journal`] keeps cumulative durability and deliveries held behind
//! their prerequisite commits; [`Counters`], jig's core's, keeps the deployment's
//! numbers. Neither knows child policy, worker placement, store encoding or IO;
//! the walking root routes internal deliveries.
//!
//! Reserve a decision before child mutation, then call [`accept_pending`] once
//! synchronous routes finish. The store answers issued commits through [`committed`] or
//! [`uncommitted`]; [`resume`] releases at most one ready delivery. Refused
//! admission returns ownership, successful admission may emit one commit, and
//! failed storage emits one stop notice. No helper waits for its own effect.
use crate::{Key, Record, Write};
use JournalOutput as Output;
use alloc::boxed::Box;
use core::mem::size_of;
pub use jig_domain_core::{Counters, fresh};
use jig_domain_people as people;
use skein_lib::{
    Decision as SkeinDecision, Journal as SkeinJournal, JournalLimits, JournalRoom, List, Queue, ReplyTo, Token,
};

/// Startup capacities supplied by the root, immutable across journal calls
/// (domain/root.md, 4 and 10). Positive commit/write/delivery room and a held
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

/// Owned root effects held after the decision they follow (domain/root.md, section 4). External notices have no acknowledgement of their own; web
/// replies consume one reply destination. Internal fleet/load deliveries are
/// resumed by the root and never passed to the protocol as child events.
#[derive(PartialEq, Eq, Debug)]
pub enum Delivery {
    /// The root hears a project made only after its decision commits.
    ProjectMade {
        project: u32,
    },
    /// Sign-in binding answer held behind the row's commit (protocol/sign-in.md, 5).
    Bound {
        request: Token,
        answer: temper_engine_domain_forge::BindAnswer,
    },
    /// Release an act answer after its core decision commits.
    DemoActed {
        to: ReplyTo,
        outcome: jig_domain_people::Outcome,
    },
    /// A secret-free diagnostic, at most 256 bytes (protocol/setup.md, 8.4).
    DemoNote {
        line: Box<[u8]>,
    },
    /// Whole forge panel, held with the decision that requested it (domain/root.md, 5.1).
    Panel {
        watcher: Token,
        panel: Box<temper_engine_domain_forge::Panel>,
    },
    /// The deployment's connection document (domain/root.md, 5.1).
    Forges {
        watcher: Token,
        forges: temper_engine_domain_forge::Forges,
    },
    SourceSignedOut {
        to: ReplyTo,
        channel: Token,
        name: jig_domain_fleet::SourceName,
        result: Option<jig_domain_fleet::SignOutResult>,
    },
    Application {
        to: ReplyTo,
        outcome: Option<crate::ApplicationAnswer>,
    },
    /// Host protocol handoffs follow their prerequisite commits.
    Host(Box<crate::HostDelivery>),
    /// One record of the hub's link to a worker channel (jig's `domain/hosts.md`, 2).
    Link {
        channel: Token,
        down: jig_domain_host::LinkDown,
    },
    /// Release a committed connector entry to its client.
    ForgeCommitted {
        entry: u64,
    },
    /// One bounded connector call after all preceding progress commits.
    ForgeCall {
        call: Token,
        repository: skein_forge::api::Repository,
        op: skein_forge::api::Op,
    },
    /// Committed request to the connector that owns a procedure task. The owner steps against
    /// current facts and returns the fenced decision through the root.
    Procedure {
        task: u64,
        step: u64,
        connector: u16,
        code: u32,
    },
    /// Committed words presented to fleet only after their task row commits.
    Relay {
        task: u64,
        attempt: u64,
        previous: Option<u64>,
        word: jig_domain_tasks::Word,
    },
    /// Root internal historical read, released after preceding commit; consumed
    /// once by root and never exposed to the shell.
    ReadEscalationDecision {
        /// Reserved root read slot, retired after IO terminal.
        waiter: Token,
    },
    /// Root to fleet after durability; the root resumes it internally.
    Fleet(
        /// Root-issued `Start`, `TurnKept`, `Acknowledge` or `Cancel` callback. `Start` workstream
        /// bytes are at most journal `transcript_bytes`; other payloads are fixed-size. Fleet
        /// start/cancel consequences route back inside the root.
        jig_domain_fleet::Event,
    ),
    /// Root to worker: complete bounded assignment after its claim commit.
    Assigned {
        /// Worker protocol destination, bounded by fleet worker room.
        channel: Token,
        /// Complete claim/brief/grant; section slots and owned bytes validated at journal
        /// admission.
        assignment: crate::Assignment,
    },
    /// Root to web: one terminal people reply, after the decision it names.
    WebReply {
        to: ReplyTo,
        reply: people::Reply,
    },
    /// Root to the clients' component: a request's carried value the core
    /// left unused, erased there before the request's answer (jig's
    /// `protocol/store.md`, 9).
    CarriedUnused {
        number: u64,
    },
    /// One authenticated read's answer, after every preceding commit.
    Read {
        reply: ReplyTo,
        answer: jig_domain_core::reads::Answer,
    },
    /// A core read's store load, after every preceding commit.
    ReadLoad {
        load: Token,
        range: Box<jig_domain_core::reads::Range>,
    },
    /// A watched subject's whole current document.
    Document {
        watcher: Token,
        document: Box<jig_domain_core::documents::Document>,
    },
    /// A watch ended after every outstanding terminal.
    WatchEnded {
        watcher: Token,
        why: jig_domain_views::End,
    },
    /// Every commit before the drain is durable.
    Drained,
    /// Root to worker: refuse its hello because of startup, shape or fleet admission; no attempt is
    /// assigned and no terminal is owed for this notice.
    Refuse {
        /// Fixed-size worker protocol destination echoed from its refused hello; no worker slot is
        /// admitted.
        channel: Token,
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
    /// Root to the task requester: live notice of its durable ending, with no reply right.
    Result {
        /// Durable person requester, checked by the task route.
        person: u64,
        task: u64,
        /// Owned result text, at most journal `result_bytes`.
        words: Box<[u8]>,
    },
}

/// Journal to its root caller: at most one value per entry point; the caller
/// translates it to store IO or an outward/internal delivery (domain/root.md, 4).
#[derive(PartialEq, Eq, Debug)]
pub enum JournalOutput {
    /// Output that decides nothing, admitted through the journal's door.
    Now(crate::boundary::Request),
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
/// payloads until accepted or returned on refusal (domain/root.md, 4).
/// This value knows no durability and emits nothing on drop.
#[derive(Debug)]
pub struct Decision {
    limits: Limits,
    reserved: JournalRoom,
    writes: List<Write>,
    deliveries: Queue<Delivery>,
    overrun: bool,
    batch: Option<SkeinDecision<Write, crate::Output>>,
}

/// skein-lib's bounded commit barrier, over the root's wrapped writes and outputs.
pub type Journal = SkeinJournal<Write, crate::Output>;

impl Decision {
    pub(crate) fn write_free(&self) -> bool {
        !self.overrun && self.writes.is_empty()
    }

    pub(crate) fn empty(&self) -> bool {
        !self.overrun && self.writes.is_empty() && self.deliveries.is_empty()
    }

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
        if reserved.writes > limits.writes || reserved.held > limits.deliveries {
            return None;
        }
        let batch = journal.decision(&reserved)?;
        assert!(worst_case(limits).is_some(), "valid root journal limits");
        Some(Decision {
            limits: *limits,
            reserved,
            // The header goes straight into the Skein batch. A read-only
            // store terminal reserves no writes, not even a header slot.
            writes: List::with_capacity(reserved.writes.saturating_sub(1)),
            deliveries: Queue::with_capacity(reserved.held),
            overrun: false,
            batch: Some(batch),
        })
    }

    /// A replacement at the same key keeps its original position. All child callbacks finish before
    /// submission, so only the final value is saved. The root supplies the write; successful
    /// admission emits no event. Bounds or capacity refusal returns it unchanged. Deployment writes
    /// are reserved for `accept`, not callers.
    #[expect(clippy::result_large_err, reason = "journal refusal returns the caller's owned bounded row")]
    pub fn write(&mut self, limits: &Limits, write: Write) -> Result<(), Write> {
        assert!(*limits == self.limits, "decision uses its configured limits");
        let within = match &write {
            Write::CopySettingsRevision(number) => *number != 0,
            Write::Save(Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Deployment(_))))
            | Write::Erase(Key::Core(jig_domain_core::Key::Core(
                jig_domain_core::CoreKey::Deployment
                | jig_domain_core::CoreKey::EscalationDecision { .. }
                | jig_domain_core::CoreKey::ProposalDecision { .. },
            ))) => false,
            Write::Save(Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Turn(row)))) => {
                row.task != 0
                    && row.attempt != 0
                    && row.turn != 0
                    && row.transcript.len() <= usize::try_from(limits.transcript_bytes).expect("u32 fits usize")
            }
            Write::Save(Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Call(row)))) => {
                row.key.task != 0 && row.key.attempt != 0 && row.key.completion != 0
            }
            Write::Erase(
                Key::Core(
                    jig_domain_core::Key::Core(
                        jig_domain_core::CoreKey::Settings(_)
                        | jig_domain_core::CoreKey::Call(_)
                        | jig_domain_core::CoreKey::Turn { .. }
                        | jig_domain_core::CoreKey::RunProof(..)
                        | jig_domain_core::CoreKey::Terminal { .. }
                        | jig_domain_core::CoreKey::Projection(_),
                    )
                    | jig_domain_core::Key::Tasks(_)
                    | jig_domain_core::Key::People(_)
                    | jig_domain_core::Key::Accounts(_),
                )
                | Key::Forge(_),
            ) => true,
            Write::Save(Record::Core(jig_domain_core::Record::Core(
                jig_domain_core::CoreRecord::EscalationDecision(row),
            ))) => {
                row.task != 0
                    && row.revision != 0
                    && row.by != 0
                    && row.requester != 0
                    && match &row.decision {
                        jig_domain_people::EscalationDecision::Release
                        | jig_domain_people::EscalationDecision::Pass => true,
                        jig_domain_people::EscalationDecision::Reject { reason } => {
                            reason.len()
                                <= usize::try_from(limits.result_bytes.min(limits.transcript_bytes))
                                    .expect("u32 fits usize")
                        }
                    }
            }
            Write::Save(Record::Core(jig_domain_core::Record::Core(
                jig_domain_core::CoreRecord::ProposalDecision(row),
            ))) => row.project != 0 && row.proposal != 0 && row.by != 0,
            Write::Erase(Key::Core(jig_domain_core::Key::Notes(_)))
            | Write::Save(
                Record::Core(
                    jig_domain_core::Record::Tasks(_)
                    | jig_domain_core::Record::Core(
                        jig_domain_core::CoreRecord::Settings(_)
                        | jig_domain_core::CoreRecord::Projection(_)
                        | jig_domain_core::CoreRecord::RunProof(_)
                        | jig_domain_core::CoreRecord::Terminal(_),
                    )
                    | jig_domain_core::Record::People(_)
                    | jig_domain_core::Record::Notes(_)
                    | jig_domain_core::Record::Accounts(_),
                )
                | Record::Forge { .. },
            ) => match crate::store::owned_bytes(&write) {
                Some(bytes) => bytes <= u64::from(limits.transcript_bytes),
                None => false,
            },
        };
        if !within {
            return Err(write);
        }
        self.carry_write(limits, write)
    }

    /// Carry the owner's wrapped write without inspecting its value.
    #[expect(clippy::result_large_err, reason = "a refused write returns ownership")]
    pub(crate) fn carry_write(&mut self, limits: &Limits, write: Write) -> Result<(), Write> {
        assert!(*limits == self.limits, "decision uses configured limits");
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
    pub fn deliver(&mut self, limits: &Limits, delivery: Delivery) -> Result<(), Delivery> {
        assert!(*limits == self.limits, "decision uses its configured limits");
        let within = match &delivery {
            Delivery::Result { words, .. } => {
                words.len() <= usize::try_from(limits.result_bytes).expect("u32 fits usize")
            }
            Delivery::Panel { panel, .. } => {
                let mut owned = panel
                    .owner
                    .len()
                    .saturating_add(size_of::<temper_engine_domain_forge::Panel>())
                    .saturating_add(panel.name.len())
                    .saturating_add(panel.branch.len())
                    .saturating_add(panel.landing.len())
                    .saturating_add(
                        panel.gates.len().saturating_mul(size_of::<temper_engine_domain_forge::PanelGate>()),
                    );
                if let temper_engine_domain_forge::PanelStep::Queued { ahead, .. } = &panel.step {
                    owned = owned.saturating_add(ahead.len().saturating_mul(size_of::<u64>()));
                }
                panel.within()
                    && owned
                        <= usize::try_from(limits.result_bytes.max(limits.transcript_bytes)).expect("u32 fits usize")
            }
            Delivery::Forges { forges, .. } => {
                let mut owned = forges.rows.len().saturating_mul(size_of::<temper_engine_domain_forge::Row>());
                for row in &forges.rows {
                    use temper_engine_domain_forge::Connection;
                    owned = owned.saturating_add(
                        row.installations
                            .len()
                            .saturating_mul(size_of::<(u64, temper_engine_domain_forge::Installation)>()),
                    );
                    for (_, installation) in &row.installations {
                        if let Some((_, login)) = &installation.account {
                            owned = owned.saturating_add(login.len());
                        }
                    }
                    if let Some(why) = &row.ended {
                        owned = owned.saturating_add(why.len());
                    }
                    owned = owned.saturating_add(match &row.connection {
                        Connection::Connected { login, .. } => login.len(),
                        Connection::SetUpFailed { why } => why.len(),
                        Connection::Stopped { reason } => reason.len(),
                        Connection::NotConnected
                        | Connection::NotSetUp
                        | Connection::SettingUp { .. }
                        | Connection::Connecting => 0,
                    });
                }
                owned <= usize::try_from(limits.result_bytes.max(limits.transcript_bytes)).expect("u32 fits usize")
            }
            Delivery::DemoNote { line } => line.len() <= 256,
            Delivery::DemoActed { outcome, .. } => match crate::store::program_answer_bytes(outcome) {
                Some(bytes) => bytes <= u64::from(limits.result_bytes.max(limits.transcript_bytes)),
                None => false,
            },
            Delivery::Fleet(event) => fleet_delivery_within(event),
            Delivery::Relay { word, .. } => {
                word.number != 0
                    && word.words.len() <= usize::try_from(limits.transcript_bytes).expect("u32 fits usize")
            }
            Delivery::Assigned { assignment, .. } => assignment_within(assignment, limits),
            Delivery::Host(host) => host_within(host, limits),
            Delivery::SourceSignedOut { .. }
            | Delivery::Link { .. }
            | Delivery::ProjectMade { .. }
            | Delivery::ForgeCommitted { .. }
            | Delivery::ForgeCall { .. }
            | Delivery::Bound { .. }
            | Delivery::Procedure { .. }
            | Delivery::Application { .. }
            | Delivery::WebReply { .. }
            | Delivery::CarriedUnused { .. }
            | Delivery::Read { .. }
            | Delivery::ReadLoad { .. }
            | Delivery::Document { .. }
            | Delivery::WatchEnded { .. }
            | Delivery::Drained
            | Delivery::Refuse { .. }
            | Delivery::ReadEscalationDecision { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Load { .. } => true,
        };
        if !within {
            return Err(delivery);
        }
        self.carry_delivery(limits, delivery)
    }

    /// Carry the owner's assembled output without inspecting its value.
    #[expect(clippy::result_large_err, reason = "a refused assembled output returns ownership")]
    pub(crate) fn carry_delivery(&mut self, limits: &Limits, delivery: Delivery) -> Result<(), Delivery> {
        assert!(*limits == self.limits, "decision uses configured limits");
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
    decision: Decision,
) -> Result<(), Decision> {
    accept_routed(journal, counters, limits, decision, true)
}

/// The cold load reads the durable header before numbering another commit.
#[expect(clippy::result_large_err, reason = "refused root decisions return all owned writes and outputs")]
pub(crate) fn accept_routed(
    journal: &mut Journal,
    counters: &mut Counters,
    limits: &Limits,
    mut decision: Decision,
    persist_header: bool,
) -> Result<(), Decision> {
    if decision.overrun {
        let mut batch = match decision.batch.take() {
            Some(batch) => batch,
            None => journal
                .decision(&JournalRoom { writes: 0, held: 0 })
                .expect("overrun follows an admitted root decision"),
        };
        for _ in 0..=limits.writes {
            if batch
                .write(Write::Erase(Key::Core(jig_domain_core::Key::Core(jig_domain_core::CoreKey::Deployment))))
                .is_err()
            {
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
    let writing = (persist_header && counters.dirty()) || !decision.writes.is_empty();
    let mut batch = match decision.batch.take() {
        Some(batch) => batch,
        None => journal.decision(&room(limits)).expect("preflight reserved the whole decision"),
    };
    if writing {
        batch
            .write(Write::Save(Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Deployment(
                counters.next_commit(),
            )))))
            .expect("header slot reserved");
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
        batch.hold(crate::boundary::delivery_output(delivery)).expect("admitted held output");
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

/// Accept the root's validated decision and take its numbered store commit.
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
    let mut released = Queue::with_capacity(1);
    let _released: skein_lib::Released = journal.release(&mut released);
    if let Some(request) = released.pop() {
        match request {
            crate::Output::Worker(crate::WorkerRequest::Deliver(delivery))
            | crate::Output::Party(crate::PartyRequest::Deliver(delivery))
            | crate::Output::ToChild(crate::Released(delivery)) => out.push(Output::Deliver(delivery)),
            crate::Output::Bound { request, answer } => {
                out.push(Output::Deliver(Delivery::Bound { request, answer }));
            }
            crate::Output::Forge { call, repository, op } => {
                out.push(Output::Deliver(Delivery::ForgeCall { call, repository, op }));
            }
            request @ (crate::Output::Worker(
                crate::WorkerRequest::Host(_) | crate::WorkerRequest::ReadResult { .. },
            )
            | crate::Output::Party(
                crate::PartyRequest::SignInRefused { .. }
                | crate::PartyRequest::WatchRefused { .. }
                | crate::PartyRequest::RunText { .. },
            )
            | crate::Output::Store(_)
            | crate::Output::Setup { .. }
            | crate::Output::StopGroup { .. }
            | crate::Output::SetUpGroup { .. }
            | crate::Output::SetUp { .. }
            | crate::Output::Account(_)
            | crate::Output::Busy(_)
            | crate::Output::Drained
            | crate::Output::Return { .. }
            | crate::Output::Connect { .. }
            | crate::Output::Manifest { .. }
            | crate::Output::Reachable { .. }
            | crate::Output::Install { .. }
            | crate::Output::EraseCarried { .. }
            | crate::Output::Stop) => out.push(Output::Now(request)),
        }
    }
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
                        List::<crate::BriefSection>::worst_case(limits.deliveries)?
                            .max(u64::try_from(size_of::<jig_domain_tasks::EscalationContext>()).ok()?),
                    )?,
            )?,
        )
}

fn fleet_delivery_within(event: &jig_domain_fleet::Event) -> bool {
    use jig_domain_fleet::Event;
    match event {
        Event::Start { .. }
        | Event::TurnKept { .. }
        | Event::Acknowledge { .. }
        | Event::Cancel { .. }
        | Event::Relayed { .. } => true,
        Event::Needs(_)
        | Event::CleanupPrograms { .. }
        | Event::CheckProgram { .. }
        | Event::SignOutProgram { .. }
        | Event::Sources { .. }
        | Event::Check { .. }
        | Event::SignOut { .. }
        | Event::Logins { .. }
        | Event::SignedOut { .. }
        | Event::Inbound { .. }
        | Event::Relay { .. }
        | Event::Adopt { .. }
        | Event::Loaded
        | Event::Grant { .. }
        | Event::Unauthorized { .. }
        | Event::Limited { .. }
        | Event::AccountRefused { .. }
        | Event::Hello { .. }
        | Event::Lost { .. }
        | Event::Answer { .. }
        | Event::Turn { .. }
        | Event::TurnBusy { .. }
        | Event::Observe { .. } => false,
    }
}

fn assignment_within(assignment: &crate::Assignment, limits: &Limits) -> bool {
    let charter_within = match crate::limits::run_charter_bytes(&assignment.run) {
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
    for word in &assignment.messages {
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
            crate::BriefBody::Text(bytes) => u64::try_from(bytes.len()).expect("usize fits u64"),
            crate::BriefBody::Missing(_) => 0,
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

fn host_within(host: &crate::HostDelivery, limits: &Limits) -> bool {
    match host {
        crate::HostDelivery::Render { name, tool, answer, .. } => {
            length_within(name.len().checked_add(tool.len()), limits.transcript_bytes)
                && bytes_within(crate::store::call_answer_bytes(answer), limits.transcript_bytes)
        }
        crate::HostDelivery::Answer { call, .. } => match call.owned_bytes() {
            Some(bytes) => bytes <= u64::from(limits.transcript_bytes),
            None => false,
        },
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

fn answered_within(assignment: &crate::Assignment, limits: &Limits) -> bool {
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
