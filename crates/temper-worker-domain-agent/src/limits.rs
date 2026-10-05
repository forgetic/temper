use alloc::boxed::Box;

use skein_lib::{Deadlines, Duration, Queue, Set, Slab, Token};

use crate::agent::{Agent, Alarm};
use crate::channel::Down;
use crate::facts::Fact;

/// The agent child domain's limits (section 7), handed by its parent to every
/// step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Agent processes at once: the process slots. A spawn beyond them is
    /// refused as busy.
    pub agents: u32,
    /// Repository descriptors admitted with a spawn.
    pub repositories: u32,
    pub name_bytes: u32,
    /// Distinct credential accounts an agent may use.
    pub accounts: u32,
    /// The most bytes of a charter.
    pub charter_bytes: u64,
    /// The most bytes of a snapshot: a spawn's, or a parked run's.
    pub snapshot_bytes: u64,
    pub transcript_bytes: u64,
    pub turn_bytes: u64,
    /// Conflict paths per repository and bytes in one relative path.
    pub conflicts: u32,
    pub path_bytes: u32,
    /// The most bytes of an inbound event. A larger one is bounced.
    pub event_bytes: u64,
    /// Inbound events queued or awaiting the run's named acknowledgement.
    /// Beyond them, an event is bounced.
    pub events: u32,
    /// Host calls a run may have in flight, from the call until its answer is
    /// sent down. A call beyond them is answered as busy.
    pub calls: u32,
    /// The most bytes of a host call's body or message.
    pub call_bytes: u64,
    /// The most bytes of an answer to a host call. A larger one goes down as
    /// too large.
    pub answer_bytes: u64,
    /// The most bytes of a fact.
    pub fact_bytes: u64,
    /// The most bytes of a run's declared outcome.
    pub outcome_bytes: u64,
    /// The most bytes of detail kept for operators: the tail of what io gives.
    pub detail_bytes: u32,
    /// How long io has to spawn an agent.
    pub spawn_timeout: Duration,
    /// How long a run may go without progress while its watchdog's clock
    /// runs.
    pub no_progress: Duration,
    /// The longest a long operation may announce. A longer one breaks the
    /// channel's rules.
    pub long_span: Duration,
    /// How long a run may live, from its spawn, whatever it is doing. Past
    /// it, the run is cancelled, then terminated and killed past the grace.
    pub wall_time: Duration,
    /// How long a run has to go on its own once it is cancelled, has said how
    /// it finishes or has exited, before its tree is terminated.
    pub grace: Duration,
    /// How long a terminated tree has before it is killed.
    pub kill_after: Duration,
    /// Facts kept until the parent drains them. Beyond them, facts are
    /// dropped and counted.
    pub facts: u32,
}

/// The most memory the domain holds under `limits`, in bytes (6.3), or `None`
/// if it does not fit a `u64`.
///
/// It counts the containers, their bookkeeping included, and the payloads, not
/// allocator overhead. An agent holds its charter and snapshot until its
/// process has spawned, then, while its run listens, the messages waiting to
/// go down, the busy answers, and the names of its calls in flight, those the
/// client has not answered and those the run withdrew; and sent event names awaiting acknowledgement; and the detail of its
/// end once its tree is empty. What comes up (host calls, facts, how the run
/// finishes) is moved into a request in the step it arrives in, and what goes
/// down is moved into a send: either is its receiver's to count.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let agents = Slab::<Agent>::worst_case(limits.agents)?;
    let alarms = Deadlines::<Alarm>::worst_case(alarms(limits)?)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    let descriptors = u64::from(limits.repositories).checked_mul(
        u64::try_from(size_of::<crate::channel::Repository>()).ok()?.checked_add(u64::from(limits.name_bytes))?,
    )?;
    let grants = u64::from(limits.accounts).checked_mul(u64::try_from(size_of::<crate::channel::Grant>()).ok()?)?;
    let spawning = limits
        .charter_bytes
        .checked_add(limits.snapshot_bytes.max(limits.transcript_bytes))?
        .checked_add(descriptors)?
        .checked_add(grants)?
        .checked_add(u64::from(limits.repositories).checked_mul(
            u64::try_from(size_of::<crate::channel::RepositoryV2>()).ok()?.checked_add(
                u64::from(limits.conflicts).checked_mul(
                    u64::try_from(size_of::<Box<[u8]>>()).ok()?.checked_add(u64::from(limits.path_bytes))?,
                )?,
            )?,
        )?)?;
    let events = u64::from(limits.events).checked_mul(limits.event_bytes)?;
    let path_bytes = u64::from(limits.conflicts)
        .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?.checked_add(u64::from(limits.path_bytes))?)?;
    let answers = u64::from(limits.calls).checked_mul(limits.answer_bytes.max(path_bytes))?;
    let names = Set::<Token>::worst_case(limits.calls)?.checked_mul(3)?;
    let busy = Queue::<Token>::worst_case(BUSY)?;
    let outbox = Queue::<Down>::worst_case(outbox(limits)?)?;
    let listening = outbox
        .checked_add(Set::<u32>::worst_case(limits.accounts)?)?
        .checked_add(Queue::<Token>::worst_case(limits.events)?)?
        .checked_add(busy)?
        .checked_add(names)?
        .checked_add(events)?
        .checked_add(answers)?;
    let agent = spawning.max(listening).checked_add(u64::from(limits.detail_bytes))?;
    let held = u64::from(limits.agents).checked_mul(agent)?;
    agents.checked_add(alarms)?.checked_add(facts)?.checked_add(held)
}

/// Busy answers that may wait to go down: one, with room kept for it, and a
/// second, behind which nothing more is read.
pub(crate) const BUSY: u32 = 2;

/// The alarm table's capacity: an agent has two alarms armed at most, its
/// watchdog and its wall time while its run is live. Its grace runs only once
/// it has left live, and `follow` cancels the other two before it arms the
/// grace, so the three are never armed together.
pub(crate) fn alarms(limits: &Limits) -> Option<u32> {
    limits.agents.checked_mul(2)
}

/// The room of a run's outbox: every inbound event that may wait, an answer
/// for every call in flight, and the cancel.
pub(crate) fn outbox(limits: &Limits) -> Option<u32> {
    limits.events.checked_add(limits.calls)?.checked_add(limits.accounts)?.checked_add(1)
}
