//! Startup bounds and ownership of protocol memory.
use crate::{agent::Channel, credentials, link::Link, relays};
use skein_lib::{Duration, Queue};
use temper_channel::{Sizes, machine::Endpoint};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub channel: temper_channel::Limits,
    pub sizes: Sizes,
    pub accounts: u32,
    pub agents: u32,
    pub relays: u32,
    /// Receiver margin for this hop; forwarded grants carry the remaining deadline.
    pub skew: Duration,
}
impl Limits {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.channel.valid()
            && self.sizes.valid()
            && self.channel.name_bytes == self.sizes.worker_name_bytes
            && self.channel.secret_bytes == self.sizes.secret_bytes
            && self.relays >= self.sizes.slots.saturating_mul(self.sizes.run_calls)
            && self.accounts >= self.sizes.grants
    }
    /// Available socket io must honor the channel's largest exact read and
    /// whole-cap room demand; also bound queued frame records below.
    #[must_use]
    pub fn fits_io(&self, io: &skein_io::Limits) -> bool {
        let room = match temper_channel::sizes::output_cap(Endpoint::WorkerLink, &self.sizes) {
            Some(cap) => cap <= io.largest_room(),
            None => false,
        };
        let sends = match temper_channel::sizes::stream_slots(Endpoint::WorkerLink, &self.sizes) {
            Some(slots) => slots <= io.sends.saturating_add(1),
            None => false,
        };
        self.valid() && io.is_usable() && io.largest_read() >= self.channel.chunk.max(8) && room && sends
    }
}
/// Protocol-owned tables, stacks, bounded pending agent sends and scratch
/// queues. Io, domain state and the loop's public queues are counted apart.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !limits.valid() {
        return None;
    }
    let link = u64::try_from(size_of::<Link>())
        .ok()?
        .checked_add(temper_channel::sizes::worst_case(Endpoint::WorkerLink, &limits.channel, &limits.sizes)?)?
        .checked_add(credentials::worst_case(limits.accounts, limits.sizes.token_bytes)?)?
        .checked_add(relays::worst_case(limits.relays)?)?;
    let agent = u64::try_from(size_of::<Channel>())
        .ok()?
        .checked_add(temper_channel::sizes::worst_case(Endpoint::WorkerAgent, &limits.channel, &limits.sizes)?)?
        .checked_add(pending(limits)?)?;
    let scratch = Queue::<temper_channel::machine::Event>::worst_case(temper_channel::machine::MAX_UP)?
        .checked_add(Queue::<skein_lib::stream::Down>::worst_case(temper_channel::machine::MAX_DOWN)?)?;
    link.checked_add(agent.checked_mul(u64::from(limits.agents))?)?
        .checked_add(scratch.checked_mul(u64::from(limits.agents).checked_add(1)?)?)?
        .checked_add(u64::from(limits.sizes.worker_name_bytes).checked_add(u64::from(limits.sizes.secret_bytes))?)?
        .checked_add(u64::from(limits.accounts).checked_mul(4)?)
}

fn pending(limits: &Limits) -> Option<u64> {
    let sizes = &limits.sizes;
    let mut bytes = 0;
    for kind in [0x281, 0x282, 0x283, 0x284, 0x285] {
        bytes = bytes.max(u64::from(temper_channel::sizes::largest(kind, sizes)?));
    }
    // The measured body bounds all byte fields. Add actual record capacities;
    // aging a grant array briefly keeps its old records beside the new ones.
    bytes
        .checked_add(
            u64::from(sizes.repositories)
                .checked_mul(u64::try_from(size_of::<temper_channel::wire::AgentRepository>()).ok()?)?,
        )?
        .checked_add(
            u64::from(sizes.endpoints)
                .checked_mul(u64::try_from(size_of::<temper_channel::wire::EndpointDescriptor>()).ok()?)?,
        )?
        .checked_add(
            u64::from(sizes.grants)
                .checked_mul(u64::try_from(size_of::<temper_channel::wire::Grant>()).ok()?.checked_mul(2)?)?,
        )
}
