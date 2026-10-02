use alloc::boxed::Box;

use temper_lib::{Deadlines, Duration, Id, Map, Queue, Set, Slab, Token};

use crate::attempt::{Attempt, Run};
use crate::call::Call;
use crate::channel::Channel;
use crate::facts::Fact;

/// The fleet's limits (programming-style.md, section 7), handed by its parent
/// to every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Workers in contact at once. A worker beyond them is turned away at
    /// its hello.
    pub workers: u32,
    /// The most runs a worker hosts at once: slots a hello says beyond them
    /// are not used, and a hello listing more runs is turned away.
    pub slots: u32,
    /// The workstreams kept per worker, for placement to prefer.
    pub workstreams: u32,
    /// The most bytes of a workstream key. A key longer is not kept from a
    /// hello, and a start naming one is refused.
    pub workstream_bytes: u32,
    /// Attempts tracked at once: waiting to be placed, out on workers, kept
    /// for an adoption, or fenced off while a worker may still host them. A
    /// start or an adoption beyond them is refused as busy, and a hello that
    /// would list more is turned away.
    pub attempts: u32,
    /// Relayed calls the parent serves at once. A call beyond them is
    /// dropped, and its run withdraws it past its own deadline.
    pub calls: u32,
    /// How long the runs of a worker whose channel was lost are kept for it
    /// to come back, and how long an attempt a worker lists, and the parent
    /// has not claimed, waits to be adopted.
    pub grace: Duration,
    /// Facts kept until the parent drains them. Beyond them, facts are
    /// dropped and counted.
    pub facts: u32,
}

/// The most memory the model holds under `limits`, in bytes
/// (programming-style.md, 6.4), or `None` if it does not fit a `u64`.
///
/// It counts the containers, their bookkeeping included, and the workstream
/// keys, not allocator overhead. What the fleet passes on (a hello's lists,
/// the parent's payloads, which it names by token) is moved into its state or
/// dropped in the step it arrives in: the keys it keeps are counted here, the
/// rest is its sender's to count.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let channels = Slab::<Channel>::worst_case(limits.workers)?
        .checked_add(Map::<Token, Id<Channel>>::worst_case(limits.workers)?)?;
    // Each worker: the attempts it hosts, and the workstream keys it holds.
    let hosts = Set::<Id<Attempt>>::worst_case(limits.slots)?;
    let keys = Set::<Box<[u8]>>::worst_case(limits.workstreams)?
        .checked_add(u64::from(limits.workstreams).checked_mul(u64::from(limits.workstream_bytes))?)?;
    let workers = u64::from(limits.workers).checked_mul(hosts.checked_add(keys)?)?;
    let attempts = Slab::<Attempt>::worst_case(limits.attempts)?
        .checked_add(Map::<(Token, Token), Id<Attempt>>::worst_case(limits.attempts)?)?
        .checked_add(Map::<Token, Run>::worst_case(limits.attempts)?)?
        .checked_add(Map::<u64, Id<Attempt>>::worst_case(limits.attempts)?)?
        .checked_add(Deadlines::<Id<Attempt>>::worst_case(limits.attempts)?)?;
    // An attempt waiting to be placed holds its workstream key, until it
    // moves to its worker's or is dropped.
    let waiting = u64::from(limits.attempts).checked_mul(u64::from(limits.workstream_bytes))?;
    let calls = Slab::<Call>::worst_case(limits.calls)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    channels.checked_add(workers)?.checked_add(attempts)?.checked_add(waiting)?.checked_add(calls)?.checked_add(facts)
}
