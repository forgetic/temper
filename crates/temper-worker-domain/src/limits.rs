use core::mem::size_of;

use skein_lib::{Deadlines, Duration, Id, Map, Queue, Slab, Token};
use temper_worker_domain_agent as agent;
use temper_worker_domain_checkout as checkout;
use temper_worker_domain_host as host;

use crate::boundary::Told;
use crate::facts::Fact;
use crate::link::{ALARMS, Alarm, Bounced, Named, Relay};
use crate::translate::SAVED;
use crate::workspace::Workspace;

/// The worker domain's limits (section 7), handed to every step read-only: its
/// child domains', each handed down to the one it bounds, and the engine
/// link's.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub host: host::Limits,
    pub checkout: checkout::Limits,
    pub agent: agent::Limits,
    /// How long runs go on once the channel to the engine is lost. Past it,
    /// the worker cancels them itself.
    pub grace: Duration,
    /// The wait before dialling the engine again after the channel is lost or
    /// a dial fails, doubling with each dial that fails, up to `redial_max`,
    /// each drawn between half of it and all of it.
    pub redial: Duration,
    pub redial_max: Duration,
    /// The run's facts kept for the engine until the protocol layer takes
    /// them. Beyond them, facts are dropped and counted.
    pub told: u32,
    /// Relays kept while the channel to the engine is down: at least as many
    /// as may wait for the engine at once (the host's slots times the calls a
    /// run may have in flight), so that none is dropped.
    pub stalled: u32,
}

/// The most memory the domain holds under `limits`, in bytes (6.3), or `None`
/// if it does not fit a `u64` or the limits cannot be honoured: the
/// child domains' own, or limits under which one child domain would refuse what
/// another passes it within its own. The host's repositories must fit the
/// checkout's, and its names the checkout's names; its runs must each find a
/// workspace and an agent (no more slots than workspaces or agent
/// processes); what it admits must be what an agent is spawned with (a
/// charter and a snapshot no larger than the agent child domain takes), and the
/// two must agree on what a run hands back (its outcome, its snapshot) and
/// on what goes down to it (an inbound event no larger than an agent takes,
/// and no more held for a run until it is live than may wait for it in its
/// agent, so that those delivered as it starts never bounce); and a run's
/// push message, as the agent child domain bounds a call, and the
/// save's must fit a commit message. The relays kept while the engine is out
/// of reach must have room for all those that may wait for it. A backoff must
/// be a wait, and no longer than its ceiling.
///
/// It is the child domains', plus what the top level keeps: a record of each
/// workspace and the map that finds those being prepared, the link's
/// alarms, the answers it keeps until the engine acknowledges them, one for
/// each slot at most, and the relays and bounces it keeps while the engine
/// is out of reach, the run's facts for the engine, the queues
/// that hold what each child domain emits in a step until it is routed, and the
/// facts. What the queued requests own is counted where they end up, and what
/// the hello holds goes out in the step that makes it.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let Limits { host: host_limits, checkout: checkout_limits, agent: agent_limits, .. } = limits;
    let fits = host_limits.repositories <= checkout_limits.repositories
        && host_limits.repositories <= agent_limits.repositories
        && host_limits.name_bytes <= agent_limits.name_bytes
        && host_limits.accounts <= agent_limits.accounts
        && host_limits.name_bytes <= checkout_limits.name_bytes
        && host_limits.slots <= checkout_limits.workspaces
        && host_limits.slots <= agent_limits.agents
        && host_limits.charter_bytes <= agent_limits.charter_bytes
        && host_limits.snapshot_bytes == agent_limits.snapshot_bytes
        && host_limits.outcome_bytes == agent_limits.outcome_bytes
        && host_limits.event_bytes <= agent_limits.event_bytes
        && host_limits.held <= agent_limits.events
        && u64::from(host_limits.slots).checked_mul(u64::from(host_limits.run_calls))? <= u64::from(limits.stalled)
        && agent_limits.call_bytes <= u64::from(checkout_limits.message_bytes)
        && len(SAVED) <= u64::from(checkout_limits.message_bytes)
        && Duration::ZERO < limits.redial
        && limits.redial <= limits.redial_max;
    if !fits {
        return None;
    }
    let children = host::worst_case(host_limits)?
        .checked_add(checkout::worst_case(checkout_limits)?)?
        .checked_add(agent::worst_case(agent_limits)?)?;
    let slots = host_limits.slots;
    let workspaces = Slab::<Workspace>::worst_case(slots)?
        .checked_add(Map::<Token, Id<Workspace>>::worst_case(slots)?)?
        .checked_add(
            u64::from(slots).checked_mul(
                u64::from(host_limits.repositories).checked_mul(
                    u64::try_from(size_of::<agent::channel::Repository>())
                        .ok()?
                        .checked_add(4)?
                        .checked_add(u64::from(host_limits.name_bytes))?,
                )?,
            )?,
        )?;
    let alarms = Deadlines::<Alarm>::worst_case(ALARMS)?;
    // An answer holds the outcome, the snapshot or the detail of a failure,
    // and the run's work: the repositories it landed in, and its save.
    let answer = host_limits
        .outcome_bytes
        .max(host_limits.snapshot_bytes)
        .max(u64::from(host_limits.detail_bytes))
        .checked_add(u64::from(host_limits.repositories).checked_mul(work()?)?)?;
    let answers = Map::<Named, host::Answer>::worst_case(slots)?.checked_add(u64::from(slots).checked_mul(answer)?)?;
    let relays = Queue::<Relay>::worst_case(limits.stalled)?
        .checked_add(u64::from(limits.stalled).checked_mul(agent_limits.call_bytes)?)?;
    let bounces = Queue::<Bounced>::worst_case(bounces(limits)?)?;
    let told = Queue::<Told>::worst_case(limits.told)?
        .checked_add(u64::from(limits.told).checked_mul(agent_limits.fact_bytes)?)?;
    let host_out = Queue::<host::Request>::worst_case(host_out(limits))?;
    let checkout_out = Queue::<checkout::Request>::worst_case(checkout_out(limits))?;
    let agent_out = Queue::<agent::Request>::worst_case(agent_out(limits))?;
    let facts = Queue::<Fact>::worst_case(facts(limits)?)?;
    children
        .checked_add(workspaces)?
        .checked_add(alarms)?
        .checked_add(answers)?
        .checked_add(relays)?
        .checked_add(bounces)?
        .checked_add(told)?
        .checked_add(host_out)?
        .checked_add(checkout_out)?
        .checked_add(agent_out)?
        .checked_add(facts)
}

/// What an answer's work holds for each repository: its place among those
/// landed in, with the last commit landed there, and its landing in the save.
fn work() -> Option<u64> {
    u64::try_from(size_of::<host::Landed>().checked_add(size_of::<host::Landing>())?).ok()
}

fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("a length fits in a u64")
}

/// Bounces kept while the engine is out of reach: as many as the events that
/// may bounce then, those that were already with the worker. No more come
/// without a channel, and the events of a run are held until it is live, or
/// wait for it in its agent.
pub(crate) fn bounces(limits: &Limits) -> Option<u32> {
    limits.host.slots.checked_mul(limits.host.held.checked_add(limits.agent.events)?)
}

/// Facts kept until the loop drains them: as many as the child domains keep.
pub(crate) fn facts(limits: &Limits) -> Option<u32> {
    limits.host.facts.checked_add(limits.checkout.facts)?.checked_add(limits.agent.facts)
}

/// The most an agent child domain's step tells its client, as its boundary
/// says: one thing for what it took, then that the agent has gone. A step for
/// one of the client's own records tells it at most one, which is never that
/// the agent has gone, but for a spawn refused at the entrance.
const TOLD: u32 = 2;

/// The most a checkout's step tells its client that leads anywhere, as its
/// boundary says: one prepare, push or save ended.
const ENDED: u32 = 1;

/// The most host steps an entry point takes that do not answer one of the
/// host's own requests: the step for the event, if the event is for the
/// host, or one for each thing the capability the event is for tells it.
const fn first_steps() -> u32 {
    if TOLD > ENDED { TOLD } else { ENDED }
}

/// The most host steps an entry point takes: the first ones, and one more for
/// each request each of those makes, whose capability may answer it at once
/// (a delivery bounced, a prepare refused, a push or a save with nothing to
/// do, a spawn refused, which the limits rule out). What the host makes of
/// such an answer leads back to it no more: an answer to the engine, a reply
/// to the run, a release.
pub(crate) const fn host_steps(limits: &Limits) -> u32 {
    first_steps().saturating_mul(host::max_out(&limits.host).saturating_add(1))
}

/// The most steps an entry point takes of each capability: the one it is for,
/// one for each request of each first host step, and two for each of the host
/// steps that follow (a release and an answer, at most).
const fn capability_steps(limits: &Limits) -> u32 {
    first_steps().saturating_mul(host::max_out(&limits.host)).saturating_mul(3).saturating_add(1)
}

/// Room for what the host emits in an entry point.
pub(crate) const fn host_out(limits: &Limits) -> u32 {
    host_steps(limits).saturating_mul(host::max_out(&limits.host))
}

/// Room for what the checkout emits in an entry point: its steps, and the
/// release of a workspace whose prepare failed.
pub(crate) const fn checkout_out(limits: &Limits) -> u32 {
    capability_steps(limits).saturating_add(1).saturating_mul(checkout::MAX_OUT)
}

/// Room for what the agent child domain emits in an entry point.
pub(crate) const fn agent_out(limits: &Limits) -> u32 {
    capability_steps(limits).saturating_mul(agent::MAX_OUT)
}

/// The most requests the child domains emit in an entry point, each routed
/// once.
pub(crate) const fn routed(limits: &Limits) -> u32 {
    host_out(limits).saturating_add(checkout_out(limits)).saturating_add(agent_out(limits))
}
