//! Retained connection state and one active conversion, checked at startup.
use skein_lib::{Deadlines, Id, Map, Queue, Set, Slab};
use temper_channel::{Sizes, machine};

use crate::{connection::Connection, listener::Worker, translate::Repository};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub connections: u32,
    pub workers: u32,
    pub repositories: u32,
    pub channel: temper_channel::Limits,
}

impl Limits {
    pub const STARTING: Limits =
        Limits { connections: 32, workers: 16, repositories: 8, channel: temper_channel::Limits::STARTING };

    /// Plaintext channel demands and frame records must fit the actual io.
    /// Tiny pings can fill the entire byte grant with eight-byte records,
    /// including records already handed below the frame machine.
    #[must_use]
    pub fn fits_io(&self, sizes: &Sizes, io: &skein_io::Limits) -> bool {
        if worst_case(self, sizes).is_none() || !io.is_usable() {
            return false;
        }
        let Some(cap) = temper_channel::sizes::output_cap(machine::Endpoint::Engine, sizes) else {
            return false;
        };
        let Some(slots) = temper_channel::sizes::stream_slots(machine::Endpoint::Engine, sizes) else {
            return false;
        };
        io.largest_read() >= self.channel.chunk.max(8)
            && io.largest_room() >= cap
            && io.sends.saturating_add(1) >= slots
    }
}

#[must_use]
pub fn worst_case(limits: &Limits, sizes: &Sizes) -> Option<u64> {
    if !limits.channel.valid()
        || !sizes.valid()
        || limits.repositories > 256
        || sizes.worker_name_bytes != limits.channel.name_bytes
        || sizes.secret_bytes != limits.channel.secret_bytes
    {
        return None;
    }
    let machine = temper_channel::sizes::worst_case(machine::Endpoint::Engine, &limits.channel, sizes)?
        .checked_add(Queue::<machine::Event>::worst_case(machine::MAX_UP)?)?
        .checked_add(Queue::<skein_lib::stream::Down>::worst_case(machine::MAX_DOWN)?)?;
    let connections = Slab::<Connection>::worst_case(limits.connections)?
        .checked_add(u64::from(limits.connections).checked_mul(machine)?)?
        .checked_add(Map::<u32, Id<Connection>>::worst_case(limits.workers)?)?
        .checked_add(Map::<u32, Id<Connection>>::worst_case(limits.workers)?)?
        .checked_add(Set::<Id<Connection>>::worst_case(limits.connections)?)?
        .checked_add(Set::<Id<Connection>>::worst_case(limits.connections)?)?
        .checked_add(Deadlines::<Id<Connection>>::worst_case(limits.connections)?)?;
    let workers = u64::from(limits.workers).checked_mul(
        u64::try_from(size_of::<Worker>())
            .ok()?
            .checked_add(u64::from(limits.channel.name_bytes))?
            .checked_add(u64::from(limits.channel.secret_bytes))?,
    )?;
    let repositories = u64::from(limits.repositories).checked_mul(
        u64::try_from(size_of::<Repository>()).ok()?.checked_add(u64::from(sizes.name_bytes).checked_mul(2)?)?,
    )?;
    connections.checked_add(workers)?.checked_add(repositories)?.checked_add(crate::translation_worst_case(sizes)?)
}
