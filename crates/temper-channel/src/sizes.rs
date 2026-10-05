//! Bounds derived from the limits of the domains, without depending on them.
use crate::machine::{Endpoint, Machine};
use crate::wire::Term;
use core::mem::size_of;
use skein_lib::{Duration, Queue};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub chunk: u32,
    pub name_bytes: u32,
    pub secret_bytes: u32,
    pub handshake: Duration,
    pub hello: Duration,
    pub ping: Duration,
    pub silence: Duration,
    pub stall: Duration,
}
impl Limits {
    pub const STARTING: Limits = Limits {
        chunk: 65_536,
        name_bytes: 64,
        secret_bytes: 64,
        handshake: Duration::from_secs(10),
        hello: Duration::from_secs(10),
        ping: Duration::from_secs(15),
        silence: Duration::from_secs(45),
        stall: Duration::from_secs(60),
    };
    #[must_use]
    pub fn valid(&self) -> bool {
        self.handshake > Duration::ZERO
            && self.hello > Duration::ZERO
            && self.ping > Duration::ZERO
            && self.silence > Duration::ZERO
            && self.stall > Duration::ZERO
            && self.chunk > 0
            && self.name_bytes <= 64
            && self.secret_bytes <= 64
            && self.silence >= self.ping.saturating_mul(3)
    }
}

/// Every count and payload byte bound is supplied by the owning protocol layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sizes {
    pub charter: u32,
    /// Version 2 body and retained transcript bounds.
    pub turn: u32,
    pub transcript: u32,
    pub turns: u32,
    pub conflicts: u32,
    pub snapshot: u32,
    pub inbound: u32,
    pub call: u32,
    pub answer: u32,
    pub outcome: u32,
    pub fact: u32,
    pub detail: u32,
    pub name_bytes: u32,
    pub worker_name_bytes: u32,
    pub secret_bytes: u32,
    pub token_bytes: u32,
    pub repositories: u32,
    pub slots: u32,
    pub workstreams: u32,
    pub grants: u32,
    pub endpoints: u32,
    pub entries: u32,
    pub inbox: u32,
    pub run_calls: u32,
    pub stalled: u32,
    pub bounces: u32,
    pub accounts: u32,
    pub facts: u32,
    pub agent_outbox: u32,
    /// Frozen bounds; callers must retain these values.
    pub terms: u32,
    pub refuse_bytes: u32,
    pub diagnostic_bytes: u32,
}
impl Sizes {
    /// Small complete configuration, useful as a base for deployment limits.
    pub const STARTING: Sizes = Sizes {
        charter: 65_536,
        turn: 65_536,
        transcript: 1_048_576,
        turns: 8,
        conflicts: 128,
        snapshot: 65_536,
        inbound: 16_384,
        call: 16_384,
        answer: 65_536,
        outcome: 65_536,
        fact: 4096,
        detail: 4096,
        name_bytes: 64,
        worker_name_bytes: 64,
        secret_bytes: 64,
        token_bytes: 4096,
        repositories: 8,
        slots: 4,
        workstreams: 32,
        grants: 8,
        endpoints: 8,
        entries: 128,
        inbox: 16,
        run_calls: 8,
        stalled: 32,
        bounces: 32,
        accounts: 8,
        facts: 32,
        agent_outbox: 16,
        terms: 32,
        refuse_bytes: 506,
        diagnostic_bytes: 512,
    };
    #[must_use]
    pub fn valid(&self) -> bool {
        self.worker_name_bytes <= 64
            && self.secret_bytes <= 64
            && self.terms == 32
            && self.refuse_bytes == 506
            && self.diagnostic_bytes == 512
    }
}
#[path = "bounds.rs"]
pub(crate) mod bounds;
pub use bounds::largest;

#[must_use]
pub fn frame(kind: u16, sizes: &Sizes) -> Option<u32> {
    frame_version(kind, sizes, 1)
}
#[must_use]
pub fn frame_version(kind: u16, sizes: &Sizes, version: u16) -> Option<u32> {
    largest_version(kind, sizes, version)?.checked_add(8)
}
/// The most frame Send records held below one whole-cap byte grant. Frames
/// are at least eight bytes; records already moved below may accumulate
/// independently of the machine's own pending-message categories.
#[must_use]
pub fn stream_slots(endpoint: Endpoint, sizes: &Sizes) -> Option<u32> {
    stream_slots_version(endpoint, sizes, 1)
}
#[must_use]
pub fn stream_slots_version(endpoint: Endpoint, sizes: &Sizes, version: u16) -> Option<u32> {
    output_cap_version(endpoint, sizes, version)?.checked_div(8)
}

/// Output bytes include headers and the opening/refusal reserve.
#[must_use]
pub fn output_cap(endpoint: Endpoint, sizes: &Sizes) -> Option<u32> {
    output_cap_version(endpoint, sizes, 1)
}
#[must_use]
pub fn output_cap_version(endpoint: Endpoint, sizes: &Sizes, version: u16) -> Option<u32> {
    let reserve = 264_u32.checked_add(10)?.checked_add(520)?.checked_add(frame_version(16, sizes, version)?)?;
    let body = match endpoint {
        Endpoint::Engine => {
            let per = frame_version(0x181, sizes, version)?
                .checked_add(sizes.inbox.checked_mul(frame_version(0x182, sizes, version)?)?)?
                .checked_add(sizes.run_calls.checked_mul(frame_version(0x184, sizes, version)?)?)?
                .checked_add(frame_version(0x183, sizes, version)?)?
                .checked_add(frame_version(0x185, sizes, version)?)?
                .checked_add(sizes.accounts.checked_mul(frame_version(0x186, sizes, version)?)?)?;
            sizes.slots.checked_mul(per)?
        }
        Endpoint::WorkerLink => frame_version(0x101, sizes, version)?
            .checked_add(sizes.slots.checked_mul(frame_version(0x102, sizes, version)?)?)?
            .checked_add(sizes.stalled.checked_mul(frame_version(0x103, sizes, version)?)?)?
            .checked_add(sizes.bounces.checked_mul(frame_version(0x104, sizes, version)?)?)?
            .checked_add(sizes.slots.checked_mul(sizes.run_calls)?.checked_mul(
                frame_version(0x106, sizes, version)?.checked_add(frame_version(0x107, sizes, version)?)?,
            )?)?
            .checked_add(sizes.facts.checked_mul(frame_version(0x105, sizes, version)?)?)?,
        Endpoint::WorkerAgent => {
            let mut largest = 0_u32;
            for kind in 0x281..=0x285 {
                largest = largest.max(frame_version(kind, sizes, version)?);
            }
            sizes.agent_outbox.checked_mul(largest)?
        }
        Endpoint::Agent => sizes
            .run_calls
            .checked_mul(frame_version(0x201, sizes, version)?.max(frame_version(0x202, sizes, version)?))?
            .checked_add(frame_version(0x207, sizes, version)?)?
            .checked_add(sizes.facts.checked_mul(frame_version(0x203, sizes, version)?)?)?
            .checked_add(sizes.accounts.checked_mul(
                frame_version(0x208, sizes, version)?.checked_add(frame_version(0x209, sizes, version)?)?,
            )?)?
            .checked_add(frame_version(0x204, sizes, version)?)?
            .checked_add(frame_version(0x205, sizes, version)?)?
            .checked_add(frame_version(0x206, sizes, version)?)?,
    };
    let extra = if version == 2 {
        match endpoint {
            Endpoint::Engine => sizes
                .slots
                .checked_mul(sizes.turns)?
                .checked_mul(frame_version(391, sizes, version)?.checked_add(frame_version(392, sizes, version)?)?)?,
            Endpoint::WorkerLink => {
                sizes.slots.checked_mul(sizes.turns)?.checked_mul(frame_version(264, sizes, version)?)?
            }
            Endpoint::Agent => sizes.turns.checked_mul(frame_version(522, sizes, version)?)?,
            Endpoint::WorkerAgent => 0,
        }
    } else {
        0
    };
    reserve.checked_add(body)?.checked_add(extra)?.checked_add(10)
}
/// Terms advertise exactly the kinds this endpoint receives after opening.
#[must_use]
pub fn terms(endpoint: Endpoint, sizes: &Sizes) -> Option<Box<[Term]>> {
    terms_version(endpoint, sizes, 1)
}
#[must_use]
pub fn terms_version(endpoint: Endpoint, sizes: &Sizes, version: u16) -> Option<Box<[Term]>> {
    let mut out = skein_lib::List::with_capacity(sizes.terms);
    for kind in kinds(version)? {
        if endpoint.receives_version(*kind, version) && *kind > 17 {
            let term = Term { kind: *kind, largest: largest_version(*kind, sizes, version)? };
            out.push(term).expect("v1 has fewer than 32 terms");
        }
    }
    Some(out.into_boxed())
}
/// A peer must advertise each kind we send once, without foreign/duplicate kinds.
#[must_use]
pub fn check_terms(endpoint: Endpoint, peer: &[Term], sizes: &Sizes) -> Option<()> {
    check_terms_version(endpoint, peer, sizes, 1)
}
#[must_use]
pub fn check_terms_version(endpoint: Endpoint, peer: &[Term], sizes: &Sizes, version: u16) -> Option<()> {
    for (index, term) in peer.iter().enumerate() {
        if term.kind <= 17 || !endpoint.sends_version(term.kind, version) {
            return None;
        }
        for prior in peer.get(..index)? {
            if prior.kind == term.kind {
                return None;
            }
        }
    }
    for kind in kinds(version)? {
        if endpoint.sends_version(*kind, version) && *kind > 17 {
            let mut found = false;
            for term in peer {
                if term.kind == *kind && term.largest >= largest_version(*kind, sizes, version)? {
                    found = true;
                }
            }
            if !found {
                return None;
            }
        }
    }
    Some(())
}
/// Intake, one body, its exact decoded arrays and owned bytes, bounded output, and state.
#[must_use]
pub fn worst_case(endpoint: Endpoint, limits: &Limits, sizes: &Sizes) -> Option<u64> {
    worst_case_version(endpoint, limits, sizes, 1)
}
#[must_use]
pub fn worst_case_version(endpoint: Endpoint, limits: &Limits, sizes: &Sizes, version: u16) -> Option<u64> {
    let cap = output_cap_version(endpoint, sizes, version)?;
    let mut body = 512_u32;
    for kind in kinds(version)? {
        body = body.max(largest_version(*kind, sizes, version)?);
    }
    let mut decoded = 0_u64;
    for kind in kinds(version)? {
        decoded = decoded.max(decoded_heap(*kind, sizes, version)?);
    }
    let state = u64::try_from(size_of::<Machine>()).ok()?;
    state
        .checked_add(u64::from(limits.chunk).checked_add(8)?)?
        .checked_add(u64::from(body))?
        .checked_add(decoded)?
        .checked_add(u64::from(cap))?
        .checked_add(Queue::<Box<[u8]>>::worst_case(output_slots_version(endpoint, sizes, version)?)?)
}
use alloc::boxed::Box;

/// Bounded frame records, derived from outstanding domain records plus opening.
#[must_use]
pub fn output_slots(endpoint: Endpoint, sizes: &Sizes) -> Option<u32> {
    output_slots_version(endpoint, sizes, 1)
}
#[must_use]
pub fn output_slots_version(endpoint: Endpoint, sizes: &Sizes, version: u16) -> Option<u32> {
    let count = match endpoint {
        Endpoint::Engine => sizes
            .slots
            .checked_mul(3_u32.checked_add(sizes.inbox)?.checked_add(sizes.run_calls)?.checked_add(sizes.accounts)?)?,
        Endpoint::WorkerLink => 1_u32
            .checked_add(sizes.slots)?
            .checked_add(sizes.stalled)?
            .checked_add(sizes.bounces)?
            .checked_add(sizes.slots.checked_mul(sizes.run_calls)?.checked_mul(2)?)?
            .checked_add(sizes.facts)?,
        Endpoint::WorkerAgent => sizes.agent_outbox,
        Endpoint::Agent => sizes
            .run_calls
            .checked_mul(2)?
            .checked_add(sizes.facts)?
            .checked_add(sizes.accounts.checked_mul(2)?)?
            .checked_add(4)?,
    };
    let extra = if version == 2 {
        match endpoint {
            Endpoint::Engine => sizes.slots.checked_mul(sizes.turns)?.checked_mul(2)?,
            Endpoint::WorkerLink => sizes.slots.checked_mul(sizes.turns)?,
            Endpoint::Agent => sizes.turns,
            Endpoint::WorkerAgent => 0,
        }
    } else {
        0
    };
    count.checked_add(extra)?.checked_add(8)
}

/// Exact kind table of the agreed version; opening and the small status are shared.
#[must_use]
pub fn largest_version(kind: u16, sizes: &Sizes, version: u16) -> Option<u32> {
    match version {
        1 => largest(kind, sizes),
        2 => match kind {
            257 => crate::wire::v2::largest_hello(sizes),
            258 => crate::wire::v2::largest_answer(sizes),
            264 => crate::wire::v2::largest_turn(sizes),
            385 => crate::wire::v2::largest_assign(sizes),
            391 | 392 => crate::wire::v2::largest_turn_name(sizes),
            513 => crate::wire::v2::largest_agent_call(sizes),
            519 => crate::wire::v2::largest_finished(sizes),
            522 => crate::wire::v2::largest_agent_turn(sizes),
            641 => crate::wire::v2::largest_agent_start(sizes),
            _ => largest(kind, sizes),
        },
        _ => None,
    }
}
fn kinds(version: u16) -> Option<&'static [u16]> {
    match version {
        1 => Some(crate::kinds::KINDS),
        2 => Some(crate::kinds::V2),
        _ => None,
    }
}
/// Unknown bodies are skipped without allocating, up to the configured frame bound.
#[must_use]
pub fn largest_body(sizes: &Sizes, version: u16) -> Option<u32> {
    let mut body = 512_u32;
    for kind in kinds(version)? {
        body = body.max(largest_version(*kind, sizes, version)?);
    }
    Some(body)
}
fn decoded_heap(kind: u16, sizes: &Sizes, version: u16) -> Option<u64> {
    if version == 2 {
        match kind {
            257 => return crate::wire::v2::heap_hello(sizes),
            258 => return crate::wire::v2::heap_answer(sizes),
            264 => return crate::wire::v2::heap_turn(sizes),
            385 => return crate::wire::v2::heap_assign(sizes),
            391 | 392 => return crate::wire::v2::heap_turn_name(sizes),
            513 => return crate::wire::v2::heap_agent_call(sizes),
            519 => return crate::wire::v2::heap_finished(sizes),
            522 => return crate::wire::v2::heap_agent_turn(sizes),
            641 => return crate::wire::v2::heap_agent_start(sizes),
            _ => {}
        }
    }
    crate::memory::decoded_heap(kind, sizes)
}

/// Bound for a machine that may negotiate either configured version. Its output
/// container is fixed before negotiation, so all components take the range maximum.
#[must_use]
pub fn worst_case_versions(
    endpoint: Endpoint,
    limits: &Limits,
    sizes: &Sizes,
    lowest: u16,
    highest: u16,
) -> Option<u64> {
    if lowest < 1 || highest > 2 || lowest > highest {
        return None;
    }
    let mut body = 512_u32;
    let mut decoded = 0_u64;
    let mut cap = 0_u32;
    let mut slots = 0_u32;
    for version in lowest..=highest {
        body = body.max(largest_body(sizes, version)?);
        cap = cap.max(output_cap_version(endpoint, sizes, version)?);
        slots = slots.max(output_slots_version(endpoint, sizes, version)?);
        for kind in kinds(version)? {
            decoded = decoded.max(decoded_heap(*kind, sizes, version)?);
        }
    }
    u64::try_from(size_of::<Machine>())
        .ok()?
        .checked_add(u64::from(limits.chunk).checked_add(8)?)?
        .checked_add(u64::from(body))?
        .checked_add(decoded)?
        .checked_add(u64::from(cap))?
        .checked_add(Queue::<Box<[u8]>>::worst_case(slots)?)
}
